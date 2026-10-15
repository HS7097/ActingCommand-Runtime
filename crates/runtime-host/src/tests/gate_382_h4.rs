// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 H-4 (test plan v2.2 §3.6): the recovery ladder's G8 and G9 rows.
//
// G8 (inventory-host key 17; req-basis-ha H-4 G8): a failed scheduled run's key goes to its
// ladder and the instance keeps one queue (Alice 10-07: one message queue per instance). While
// the ladder holds the instance the policy defers it and writes nothing, status shows the hold,
// and the next dispatch follows the ladder's own release (model-369 v3.1 §6 steps 2, 5 and 6;
// H-1, W-1, S-1). Steps 1, 3 and 4 are the incident replay in `instance_workers`.
//
// G9 (inventory-host key 22; req-basis-ha H-4 G9): an install drain or a shutdown during rung 3's
// readiness wait (R4) ends the wait at its holding check. Rung 3 fails with the interruption, the
// climb ends exhausted at Warning (`recovery_ladder_interrupted` caused by the interruption) and
// the ladder releases its key once (model-369 H-3 holding checks, ruling Q8, §5.2, §5.5). The
// pause row is `instance_workers::`
// `a_pause_during_the_readiness_wait_ends_the_climb_at_warning_with_one_release`. "Within one
// poll on the manual clock" is not asserted: R4 polls on the monotonic wall clock, and the clock
// seam (E-3 I3) is not built yet.
//
// Every wait returns as soon as the awaited emulator call or ledger record arrives; `GATE_WAIT`
// only bounds a broken run.

use super::emulator_controller::EmulatorCall;
use super::instance_workers::{
    LadderRun, admit_ladder_run, admit_ladder_run_at, all_events, closed_ledger_events,
    emulator_ladder_fixture, ladder_end_code, ladder_finished, ladder_releases, ladder_setup,
    rung_outcomes, wait_for_ladder_end,
};
use super::*;
use actingcommand_contract::{
    EmulatorInstanceAction, FactPayload, InstallTransitionAction, RecoveryLadderOutcome,
    RecoveryRung, RecoveryRungOutcome, RuntimePayload,
};
use actingcommand_policy::SchedulingDecisionState;

const GATE_WAIT: Duration = Duration::from_secs(120);
const START: EmulatorInstanceAction = EmulatorInstanceAction::Start;
/// The policy driver's re-evaluation after the failed run (§6 step 2, W-1 review M-6), at the
/// trigger's next occurrence. The fixture task's trigger is any of an hourly clock (every
/// 3 600 000 ms from 0) and a `resource.primary` fact this fixture never records. The run's
/// dispatch served the occurrence at the fixture's instant (an exact hour), so the candidate is
/// due again from the next one, one hour later; before it the trigger is unknown and there is no
/// candidate to defer. The occurrence is past the task's retry backoff (30 s) and the activity's
/// longest interval (300 s), inside the same activity window (08:00-22:00 UTC; the fixture's
/// instant is 12:00 UTC), and far after every record the run wrote.
const RE_EVALUATION_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;
/// The next dispatch (§6 step 5): one default policy cadence cooldown (1 s) after the
/// re-evaluation, at the same occurrence, which no dispatch has served yet.
const NEXT_DISPATCH_UNIX_MS: u64 = RE_EVALUATION_UNIX_MS + 1_000;

/// A Runtime clock that moves 1 ms forward at every reading, from the fixture's instant. The
/// run's records get strictly increasing times, as on a real clock (a fact observed at the same
/// instant as the one before it, from another source, is refused `fact_observation_not_newer`),
/// and the test places each evaluation after every record it reads. It never reads the wall
/// clock.
struct SteppingClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
}

impl SteppingClock {
    fn new(unix_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
        }
    }

    /// The time of the latest reading.
    fn latest_unix_ms(&self) -> u64 {
        self.unix_ms.load(Ordering::SeqCst)
    }

    /// Moves the clock forward by `duration_ms`.
    fn advance(&self, duration_ms: u64) {
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }
}

impl RuntimeClock for SteppingClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.fetch_add(1, Ordering::SeqCst) + 1,
            monotonic_ms: self.monotonic_ms.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }
}

/// Returns once the scripted emulator sees `expected`; the calls before it are passed over.
fn await_call(calls: &mpsc::Receiver<EmulatorCall>, expected: EmulatorCall) {
    loop {
        let call = calls
            .recv_timeout(GATE_WAIT)
            .unwrap_or_else(|error| panic!("waiting for {expected:?}: {error}"));
        if call == expected {
            return;
        }
    }
}

/// Returns once rung 3's readiness wait (R4) made its first boot check after the Start. From
/// then on the ladder holds its key inside that wait, polling an Android that has not booted.
fn await_readiness_wait(calls: &mpsc::Receiver<EmulatorCall>) {
    await_call(calls, EmulatorCall::Control(START));
    await_call(calls, EmulatorCall::ObserveForeground);
}

/// Makes the fake hold the emulator's next Stop (`block_emulator_stop`) and returns the
/// receiver that hears once that Stop is held.
fn hold_next_stop(state: &FakeState) -> mpsc::Receiver<()> {
    let (held, heard) = mpsc::channel();
    *state
        .emulator_stop_held
        .lock()
        .expect("fake emulator stop held lock") = Some(held);
    state.block_emulator_stop.store(true, Ordering::Release);
    heard
}

/// Returns once rung 3's emulator Stop has reached the fake, where `block_emulator_stop` holds
/// it: rungs 1 and 2 have run, and the Stop's invalidation of the self-check (Workflow #317 sc1)
/// has not happened yet.
fn await_held_stop(heard: &mpsc::Receiver<()>) {
    heard
        .recv_timeout(GATE_WAIT)
        .unwrap_or_else(|error| panic!("rung 3's Stop is held: {error}"));
}

/// The operator's self-check of the policy instance (`SelfCheckInstance`), on `connection`: its
/// preparation opens input and capture, and the self-check passes.
fn self_check_passes(host: &RuntimeHost, connection: u64) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::SelfCheckInstance {
                    instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                },
            ),
            ConnectionId::new(connection).expect("connection"),
        )
        .expect("self-check receipt");
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    assert!(
        selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none(),
        "the instance's self-check passes: {receipt:?}"
    );
}

/// Every `session.instance.available` publication of the policy instance, in ledger order: its
/// value and its source snapshot (`backend_selfcheck:<entry>:<generation>` or
/// `backend_selfcheck:unchecked` for a self-check's withdrawal, Workflow #317 sc3).
fn availability_publications(host: &RuntimeHost) -> Vec<(bool, String)> {
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    let events = all_events(host);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => Some(payload.record()),
            _ => None,
        })
        .filter(|record| {
            record.key == crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY && record.scope == scope
        })
        .filter_map(|record| match &record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some((*value, record.source_snapshot_id.clone())),
            _ => None,
        })
        .collect()
}

/// The policy instance's `lease_active` and `queued_request_count`, as `Status` reports them.
fn lease_status(host: &RuntimeHost, connection: u64) -> (bool, u32) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(connection).expect("connection"),
        )
        .expect("status receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    let instance = status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == POLICY_INSTANCE_ALIAS)
        .expect("the instance's status");
    (instance.lease_active(), instance.queued_request_count())
}

/// Declares the governance identity on `connection` and begins an install drain, as the
/// installer does; the drain is accepted.
fn begin_install_drain(host: &RuntimeHost, connection: u64) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection = ConnectionId::new(connection).expect("connection");
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
    let declared = host
        .process_request_for_test(
            &governance(RuntimeOperation::DeclareGovernanceIdentity {
                card: test_governance_card(),
            }),
            connection,
        )
        .expect("declaration");
    assert_eq!(
        declared.state(),
        RuntimeReceiptState::Completed,
        "{declared:?}"
    );
    let begin = host
        .process_request_for_test(
            &governance(RuntimeOperation::InstallTransition {
                target: host.runtime_info().shutdown_target(),
                action: InstallTransitionAction::BeginDrain {
                    transition_id: "h4-drain-during-readiness-wait".to_owned(),
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
}

/// Ledger revision 3 (#378 A3): the code of the first cause of the ladder end's outcome. An
/// interrupted climb's `recovery_ladder_interrupted` is caused by the interruption.
fn ladder_end_cause(event: &PersistedEvent) -> Option<&str> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => value
            .outcome()
            .and_then(|outcome| outcome.causes().first())
            .map(|link| link.outcome().code().as_str()),
        _ => None,
    }
}

/// G9's end of a climb whose readiness wait `reason` interrupted (H-3, ruling Q8): rung 3, the
/// rung that was waiting, is the last rung and has one record, `failed` with `reason`; the climb
/// is exhausted at Warning with `recovery_ladder_interrupted` caused by `reason` (no error point,
/// §5.10); the ladder's claim releases its key once, after the end.
fn assert_wait_interrupted(events: &[PersistedEvent], run_lease: LeaseId, reason: &str) {
    let rungs = rung_outcomes(events);
    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some(reason.to_owned())
        )),
        "{rungs:?}"
    );
    assert_eq!(
        rungs
            .iter()
            .filter(|(rung, _, _)| *rung == RecoveryRung::EmulatorRestart)
            .count(),
        1,
        "one record of rung 3: {rungs:?}"
    );
    let (finished, outcome, _) = ladder_finished(events).expect("the ladder's end");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(finished.severity(), EventSeverity::Warning);
    assert_eq!(
        ladder_end_code(finished),
        Some("recovery_ladder_interrupted")
    );
    assert_eq!(ladder_end_cause(finished), Some(reason));
    let releases = ladder_releases(events, run_lease);
    let [release] = releases.as_slice() else {
        panic!("the ladder releases its key once: {}", releases.len());
    };
    assert!(
        release.sequence() > finished.sequence(),
        "the release follows the ladder's end"
    );
}

/// G8: model-369 §6 steps 2, 5 and 6 (H-1, W-1, S-1; inventory-host G8). The scheduled run
/// fails, and its lease end hands the key to the ladder. The driver's re-evaluation, at the
/// trigger's next occurrence while the ladder still holds the instance, then defers the due
/// candidate `dispatch_instance_held`, naming the ladder as the holder, and writes no intent.
/// Status reads `lease_active` with at most the ladder's continuation queued, and none while
/// rung 3 waits for readiness. Until the ladder's own release nothing is dispatched, rejected or
/// granted on the instance; after it the next dispatch is admitted and granted. The Runtime clock
/// steps 1 ms a reading from the fixture's instant (and jumps to the next dispatch once the
/// ladder has released), so the evaluations never precede the records they read.
///
/// The instance stays available through the re-evaluation, as in the model's incident (the run
/// was dispatched to a device whose self-check passed). Model-369 v3.1 §3.3 W-1 defers a held
/// instance's candidates `dispatch_instance_held`; an instance projected unavailable has no
/// candidate and is blocked at placement `instance_unavailable` (contracts/scheduling/README.md,
/// #317 sc3). Under sc3 a physical instance is available only while its stored self-check passes,
/// which needs `nemu`, or both `input` and `capture`, `passed`. The instance gets its emulator
/// binding (and with it the self-check gate) after the start, so the start prepared nothing, and
/// the scheduled run, failing at its first observation, opens only its capture: alone that leaves
/// `input` missing and withdraws availability (`backend_selfcheck:capture:<generation>`). So an
/// operator self-check passes first (input and capture). Rung 3's emulator Stop drops the
/// self-check and withdraws availability again (sc1), so the fake holds that Stop until the
/// re-evaluation and the status read are done: the ladder holds its key there, after rungs 1
/// and 2, with the self-check intact.
#[test]
fn gate_a_failed_runs_ladder_holds_its_instance_and_the_next_dispatch_follows_its_release() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let clock = Arc::new(SteppingClock::new(POLICY_NOW_UNIX_MS));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    // Rung 3 restarts the emulator and then waits for an Android that has not booted, so the
    // ladder holds the instance until the test lets it boot.
    state.hold_android_boot.store(true, Ordering::Release);
    let registered = instance_id();
    let host = RuntimeHost::start(
        host_config.with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, registered, &state);
    let calls = state.emulator.listen();
    state.observed_open.store(true, Ordering::Release);
    self_check_passes(&host, 220);
    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);
    let held_stop = hold_next_stop(&state);
    let failure = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!failure.is_fatal(), "{}", failure.code());
    await_held_stop(&held_stop);

    // §6 step 2 (W-1): the re-evaluation defers the held instance's due candidate with
    // `dispatch_instance_held`, the ladder as its holder (H-1), and writes no intent.
    assert!(
        clock.latest_unix_ms() < RE_EVALUATION_UNIX_MS,
        "the run's records precede the re-evaluation"
    );
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: RE_EVALUATION_UNIX_MS,
                monotonic_ms: RE_EVALUATION_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("the re-evaluation after the failed run");
    let evaluation = cycle.evaluation.as_ref().expect("the re-evaluation ran");
    assert!(
        cycle
            .pending_dispatch_intents
            .iter()
            .chain(&evaluation.dispatch_intents)
            .all(|intent| intent.instance_id != POLICY_INSTANCE_ALIAS),
        "no intent for the held instance: {:#?}",
        cycle.pending_dispatch_intents
    );
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == context.catalog_task_id()
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .expect("the failed task's decision on its instance");
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}\navailability publications (value, source): {:#?}",
        availability_publications(&host)
    );
    let held = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "dispatch_instance_held")
        .unwrap_or_else(|| panic!("deferred as held: {decision:#?}"));
    assert_eq!(
        held.outcome.value("holder_kind"),
        Some(&serde_json::json!("ladder")),
        "{held:#?}"
    );

    // §6 step 6 (S-1): the hold is a lease, with at most the ladder's continuation waiting
    // during a rung run, and nothing waiting during rung 3's readiness wait (review2 L2).
    let (lease_active, queued) = lease_status(&host, 221);
    assert!(lease_active, "the ladder's hold reads as an active lease");
    assert!(
        queued <= 1,
        "at most the ladder's continuation waits: {queued}"
    );
    state.block_emulator_stop.store(false, Ordering::Release);
    await_readiness_wait(&calls);
    assert_eq!(
        lease_status(&host, 222),
        (true, 0),
        "rung 3's readiness wait holds the lease and queues nothing"
    );

    // Android boots, and the fake's opens pass a preparation's self-check: the readiness wait's
    // next attempt passes, rung 3 runs the startup package on the ladder's key, and the climb
    // ends with the ladder's release.
    state.android_down.store(false, Ordering::Release);
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    // §6 step 5: after the ladder's release the next dispatch on the instance is admitted. The
    // Runtime clock moves to it only now, so the ladder's hold never lapsed on it.
    assert!(
        clock.latest_unix_ms() < NEXT_DISPATCH_UNIX_MS,
        "the climb's records precede the next dispatch"
    );
    clock.advance(NEXT_DISPATCH_UNIX_MS - clock.latest_unix_ms());
    admit_ladder_run_at(&host, &request, clock.latest_unix_ms());

    let events = all_events(&host);
    let run_id = context.run_id();
    let run_release = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().run_id() == Some(&run_id)
        })
        .expect("the failed run's release");
    let releases = ladder_releases(&events, context.lease_token().lease_id());
    let [ladder_release] = releases.as_slice() else {
        panic!("the ladder releases its key once: {}", releases.len());
    };
    let climb = run_release.sequence()..ladder_release.sequence();
    let during_climb = events
        .iter()
        .filter(|event| climb.contains(&event.sequence()))
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::LeaseGranted
                    | EventType::PolicyDispatchIntent
                    | EventType::PolicyDispatchRejected
            )
        })
        .collect::<Vec<_>>();
    assert!(
        during_climb.is_empty(),
        "nothing is dispatched, rejected or granted on the instance during the climb: \
         {during_climb:#?}"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.sequence() > ladder_release.sequence()
        }),
        "the next dispatch is granted after the ladder's release"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// G9, drain row (model-369 H-3 holding checks, ruling Q8, §5.2): an install drain begun while
/// rung 3 waits for readiness (R4) ends the wait. Rung 3 fails with
/// `recovery_ladder_drain_requested`, the climb ends exhausted at Warning, the ladder releases
/// its key once and the Runtime is not fatal. The reason, not `recovery_android_not_booted`,
/// shows that the drain and not the end of the readiness window ended the wait.
#[test]
fn gate_an_install_drain_during_the_readiness_wait_fails_rung_three_and_releases_once() {
    let run = emulator_ladder_fixture(0, true);
    let calls = run.state.emulator.listen();
    run.fail_scheduled_run();
    await_readiness_wait(&calls);
    begin_install_drain(&run.host, 223);
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    let events = all_events(&run.host);
    assert_wait_interrupted(
        &events,
        run.context.lease_token().lease_id(),
        "recovery_ladder_drain_requested",
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// G9, shutdown row (model-369 H-3, review L3, W-2 shutdown order, ruling Q8): a host close while
/// rung 3 waits for readiness (R4) ends the wait. Rung 3 fails with
/// `recovery_ladder_shutdown_requested`, the climb ends exhausted at Warning, the ladder releases
/// its key once, the close succeeds and no record is fatal.
#[test]
fn gate_a_shutdown_during_the_readiness_wait_fails_rung_three_and_releases_once() {
    let run = emulator_ladder_fixture(0, true);
    let calls = run.state.emulator.listen();
    run.fail_scheduled_run();
    await_readiness_wait(&calls);
    let run_lease = run.context.lease_token().lease_id();
    let LadderRun { root, host, .. } = run;
    host.close()
        .expect("close the host during the readiness wait");
    let events = closed_ledger_events(root.path());
    assert_wait_interrupted(&events, run_lease, "recovery_ladder_shutdown_requested");
    assert!(
        events
            .iter()
            .all(|event| event.severity() != EventSeverity::Fatal),
        "no fatal record"
    );
}
