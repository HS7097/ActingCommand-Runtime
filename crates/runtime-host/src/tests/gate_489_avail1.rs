// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #489 AVAIL-1 (0.12.76; register §1.493; `contracts/scheduling/README.md`, "An
//! unavailable instance is probed again"). The 11-20 production defect: a scheduled run's backend
//! open failed, its device self-check failed with it, the backend self-check gate withdrew the
//! instance's availability, and the ladder its failure matched was suppressed by an open cool-down
//! window; nothing evaluated the instance again for 15 hours (and a second instance, whose
//! cool-down had 27.5 s left, for 16), although a self-check passed at once when a person ran one.
//! Requirements (the coordinator's work order of 11-20, tests 1 to 5):
//!
//! - T1: the replay, in both shapes: a failed capture open, the ladder suppressed by its cool-down
//!   (30 minutes left; 27.5 s left), the instance unavailable; at the cool-down's end the
//!   instance's self-check runs: passing, it is available again and its due task is dispatched;
//!   failing, the recovery the cool-down held back is admitted and climbs;
//! - T2: with no cool-down, the instance is probed at the first interval, then at intervals
//!   multiplied by the factor up to the cap; the third probe passes, the instance is available and
//!   nothing probes it any more;
//! - T3: a paused, a fenced and an idle-stopped instance are not probed;
//! - T4: the attention is raised once when the unavailability lasted the configured time, not at
//!   every probe, and it ends with the unavailability;
//! - T5: the status of the unavailable instance carries the member `unavailable`; the status of the
//!   instance once available again is, byte for byte, what it was before.
//!
//! Every host runs on a manual Runtime clock that only the test moves; each sweep tick of the probes
//! is run by the test as well as by the lease sweep, so the waits below only bound a broken run.
//! The instance is the policy fixture's on a bound device endpoint (`with_device_endpoints`), so the
//! backend self-check gates its availability; its self-check fails by its capture open, which fails
//! after closing the one resource it took (confirmed), as the 10-18 failure of gate_0121_a7 does.

use super::gate_480_pg2b::instance_pause_as;
use super::instance_workers::{
    admit_ladder_run, admit_ladder_run_at, all_events, ladder_setup, phases, wait_for_ladder_end,
    wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactPayload, InstanceStuckRecovery, InstanceUnavailable, RecoveryLadderSuppression,
    RecoveryTriggerStage, RunId, RuntimeConfigManifest, RuntimeInstanceStatus,
    RuntimeLifecyclePhase, RuntimePayload, SchedulingPauseMode, SchedulingResumeSelfCheck,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const CAPTURE_OPEN_FAILED: &str = "capture_backend_open_failed";
/// The connection the test's status reads come on.
const STATUS_CONNECTION: u64 = 4_891;
/// The fixture task's next occurrence after the run at the fixture's instant, plus one default
/// policy cadence (gate_382_h4's `NEXT_DISPATCH_UNIX_MS`).
const NEXT_DISPATCH_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000 + 1_000;

/// A configuration manifest of the defaults file with the sections `file` names, as actingd
/// assembles it.
fn manifest(file: &Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// The capture open fails fatally after closing the one resource it acquired (confirmed).
fn capture_open_failure() -> DeviceError {
    DeviceError::fatal("fixture capture open got no positive connection handle")
        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

fn now_unix_ms(clock: &ManualRuntimeClock) -> u64 {
    clock.sample().expect("manual clock").unix_ms
}

/// The policy instance's status, as `Status` reports it.
fn instance_status(host: &RuntimeHost) -> RuntimeInstanceStatus {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(&ids, RuntimeOperation::Status),
            ConnectionId::new(STATUS_CONNECTION).expect("connection"),
        )
        .expect("status receipt");
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == POLICY_INSTANCE_ALIAS)
        .expect("the instance's status")
        .clone()
}

fn unavailable(host: &RuntimeHost) -> Option<InstanceUnavailable> {
    instance_status(host).unavailable().cloned()
}

/// The outcomes of the `runtime.failed` records whose code is `code`, in ledger order.
fn failed_outcomes(events: &[PersistedEvent], code: HostCode) -> Vec<(&PersistedEvent, &Outcome)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == code.as_str() =>
            {
                Some((event, record.outcome()))
            }
            _ => None,
        })
        .collect()
}

fn records(host: &RuntimeHost, code: HostCode) -> usize {
    failed_outcomes(&all_events(host), code).len()
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

/// The `instance_preparation_finished` records of the Runtime's own recovery preparation (a probe
/// is one; a person's self-check is a connection preparation).
fn recovery_preparations(host: &RuntimeHost) -> usize {
    phases(&all_events(host))
        .iter()
        .filter(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::RecoveryPreparation,
                    ..
                }
            )
        })
        .count()
}

/// The value of the policy instance's latest `session.instance.available` publication.
fn last_availability(host: &RuntimeHost) -> Option<bool> {
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload)) => Some(payload.record().clone()),
            _ => None,
        })
        .filter(|record| {
            record.key == crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY && record.scope == scope
        })
        .filter_map(|record| match record.content {
            FactContent::Inline {
                value: ContractFactValue::Boolean(value),
            } => Some(value),
            _ => None,
        })
        .next_back()
}

/// A host whose policy instance its device self-check made unavailable, on a manual clock at the
/// fixture's instant. Its start's preparation saw no passed open (`observed_open` false), so the
/// start withdrew its availability (`backend_selfcheck:unchecked`) and nothing republished it;
/// then a person's self-check failed by its capture open. Stuck recovery is off, so no ladder
/// starts. `file` holds the sections of its configuration besides the defaults.
struct Probed {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    registered: InstanceId,
}

impl Probed {
    fn start(file: &Value) -> Self {
        let probed = Self::started(file);
        probed.fail_self_check();
        probed
    }

    /// The host, before the person's self-check.
    fn started(file: &Value) -> Self {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let registered = instance_id();
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_config_manifest(manifest(file))
                .with_stuck_recovery(BTreeMap::from([(
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    InstanceStuckRecovery {
                        enabled: false,
                        cooldown_secs: 600,
                    },
                )])),
            Arc::new(
                FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(&state))
                    .with_device_endpoints(),
            ),
        )
        .expect("runtime host");
        Self {
            root,
            host,
            state,
            clock,
            registered,
        }
    }

    /// A person's self-check of the instance whose capture open fails: it names the open.
    fn fail_self_check(&self) {
        self.state.observed_open.store(true, Ordering::Release);
        *self
            .state
            .capture_open_error
            .lock()
            .expect("capture open error") = Some(capture_open_failure());
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let receipt = self
            .host
            .process_request_for_test(
                &runtime_request(
                    &ids,
                    RuntimeOperation::SelfCheckInstance {
                        instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                    },
                ),
                ConnectionId::new(4_890).expect("connection"),
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
    }

    /// From now on the instance's opens pass.
    fn passes(&self) {
        *self
            .state
            .capture_open_error
            .lock()
            .expect("capture open error") = None;
    }

    fn now_unix_ms(&self) -> u64 {
        now_unix_ms(&self.clock)
    }

    /// Moves the clock to the next probe's time, as the status names it.
    fn advance_to_next_probe(&self) {
        let next = unavailable(&self.host)
            .and_then(|unavailable| unavailable.next_probe_unix_ms)
            .expect("a next probe");
        self.clock.advance(next.saturating_sub(self.now_unix_ms()));
    }

    fn tick(&self) {
        self.host
            .unavailable_probe_tick_for_test()
            .expect("sweep tick");
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("runtime health").is_none());
        self.host.close().expect("close host");
        drop(self.root);
    }
}

/// The probe keys `first_interval_ms`, `factor`, `max_interval_ms` and `attention_after_ms`.
fn probe_keys(first: u64, factor: u32, max: u64, attention: u64) -> Value {
    json!({
        "scheduling": {
            "unavailable_probe": {
                "first_interval_ms": first,
                "factor": factor,
                "max_interval_ms": max,
                "attention_after_ms": attention,
            }
        }
    })
}

/// T1's fixture: the ladder fixture's scheduled run on the policy instance, on a bound device
/// endpoint, admitted while its self-check passes, then failed by its capture open while the
/// ladder's cool-down window has `cooldown_left_ms` left (the window an earlier ladder opened,
/// opened here by the test). The probes' first interval and cap are an hour, so the first probe is
/// the cool-down's end's.
struct Replay {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    request: ContainedTaskRequest,
    run_id: RunId,
    until_unix_ms: u64,
}

fn replay(cooldown_left_ms: u64) -> Replay {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    // The start withdraws the instance's availability at the clock and its passing preparation
    // republishes the seed: the preparation's first frame takes the clock's millisecond, as a real
    // open takes time, so the two are not observed at the same instant (gate_ord1c, fix round 3).
    let stepped = Arc::clone(&clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || stepped.advance(1)));
    let host = RuntimeHost::start(
        host_config
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest(&probe_keys(3_600_000, 2, 3_600_000, 3_600_000))),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .with_device_endpoints(),
        ),
    )
    .expect("physical runtime host");
    let context = admit_ladder_run(&host, &request);
    clock.advance(1);
    let until_unix_ms = now_unix_ms(&clock) + cooldown_left_ms;
    host.open_recovery_cooldown_for_test(POLICY_INSTANCE_ALIAS, until_unix_ms)
        .expect("the ladder's cool-down window");
    *state.capture_open_error.lock().expect("capture open error") = Some(capture_open_failure());
    let failure = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!failure.is_fatal(), "{}", failure.code());
    let replay = Replay {
        root,
        host,
        state,
        clock,
        request,
        run_id: context.run_id(),
        until_unix_ms,
    };
    replay.assert_held_back();
    replay
}

impl Replay {
    /// The 11-20 state: one `recovery_ladder_suppressed` `cooldown` of the trigger item
    /// `task_capture_backend` until the window's end, no ladder, and the instance unavailable for
    /// its failed capture open, its next probe at the cool-down's end.
    fn assert_held_back(&self) {
        let events = all_events(&self.host);
        let suppressed = phases(&events)
            .into_iter()
            .filter_map(|(_, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                    reason,
                    until_unix_ms,
                    rule,
                } => Some((reason, until_unix_ms, rule)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            suppressed,
            vec![(
                RecoveryLadderSuppression::Cooldown,
                self.until_unix_ms,
                Some("task_capture_backend".to_owned())
            )]
        );
        assert!(
            !phases(&events).iter().any(|(_, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
            )),
            "the cool-down suppressed the ladder"
        );
        assert_eq!(last_availability(&self.host), Some(false));
        let held = unavailable(&self.host).expect("the instance is unavailable");
        assert_eq!(
            held.cause.as_ref().map(|cause| cause.as_str()),
            Some(CAPTURE_OPEN_FAILED)
        );
        assert_eq!(held.probes, 0);
        assert_eq!(
            held.next_probe_unix_ms,
            Some(self.until_unix_ms),
            "the next probe is the cool-down's end"
        );
    }

    fn now_unix_ms(&self) -> u64 {
        now_unix_ms(&self.clock)
    }
}

/// T1, the first instance's shape (the cool-down had 30 minutes left): at the cool-down's end the
/// self-check runs and passes; the instance is available again, its unavailability ends with
/// `instance_unavailable_probe_passed`, the held recovery is dropped (no ladder starts), and the
/// due task is dispatched and granted at its next occurrence.
#[test]
fn avail1_t1_at_the_cooldowns_end_a_passing_self_check_makes_the_instance_available_again() {
    let replay = replay(1_800_000);
    let host = &replay.host;
    *replay
        .state
        .capture_open_error
        .lock()
        .expect("capture open error") = None;
    replay
        .clock
        .advance(replay.until_unix_ms.saturating_sub(replay.now_unix_ms()));
    host.unavailable_probe_tick_for_test().expect("sweep tick");
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None, "the instance is available again");
    assert_eq!(last_availability(host), Some(true));
    let events = all_events(host);
    let passed = failed_outcomes(&events, HostCode::InstanceUnavailableProbePassed);
    let [(_, passed)] = passed.as_slice() else {
        panic!("one passing probe: {passed:#?}");
    };
    assert_eq!(
        (value(passed, "attempts"), value(passed, "failure_code")),
        (json!(1), json!(CAPTURE_OPEN_FAILED))
    );
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 0);

    // The due task is dispatched and granted at its next occurrence. A capacity sample is fresh
    // for twice the monitor's interval on the Runtime clock, so it is sampled at the moved clock.
    replay
        .clock
        .advance(NEXT_DISPATCH_UNIX_MS.saturating_sub(replay.now_unix_ms()));
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("capacity sample at the next dispatch");
    let next = admit_ladder_run_at(host, &replay.request, NEXT_DISPATCH_UNIX_MS);
    assert_ne!(next.run_id(), replay.run_id);
    assert!(
        !phases(&all_events(host))
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. })),
        "a passing self-check drops the recovery the cool-down held back"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    replay.host.close().expect("close host");
    drop(replay.root);
}

/// T1, the second instance's shape (the cool-down had 27.5 s left, nothing after it): at the
/// cool-down's end the self-check runs and fails; `instance_unavailable_probe_failed` records it,
/// and the recovery the cool-down held back is admitted after it, as without the cool-down: its
/// ladder starts with the failed run's trigger (stage task, the run, the failed capture open, the
/// item `task_capture_backend`), and no second suppression is recorded.
#[test]
fn avail1_t1_at_the_cooldowns_end_a_failing_self_check_admits_the_held_recovery() {
    let replay = replay(27_500);
    let host = &replay.host;
    // Business capacity is sampled a millisecond before the cool-down's end, at the moved clock,
    // so the held recovery's admission reads a fresh sample.
    replay.clock.advance(
        replay
            .until_unix_ms
            .saturating_sub(1)
            .saturating_sub(replay.now_unix_ms()),
    );
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("capacity sample before the cool-down's end");
    host.unavailable_probe_tick_for_test().expect("sweep tick");
    assert_eq!(
        records(host, HostCode::InstanceUnavailableProbeFailed),
        0,
        "no probe before the cool-down's end"
    );
    replay.clock.advance(1);
    host.unavailable_probe_tick_for_test().expect("sweep tick");
    let run_id = replay.run_id;
    let started = || {
        phases(&all_events(host))
            .into_iter()
            .find_map(|(event, phase)| match phase {
                RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. }
                    if trigger.run_id == Some(run_id) =>
                {
                    Some((event.sequence(), trigger))
                }
                _ => None,
            })
    };
    wait_until("the held recovery's ladder", || started().is_some());
    let (started_at, trigger) = started().expect("the held recovery's ladder");
    assert_eq!(trigger.stage, RecoveryTriggerStage::Task);
    assert_eq!(trigger.failure_code.as_str(), CAPTURE_OPEN_FAILED);
    assert_eq!(trigger.rule.as_deref(), Some("task_capture_backend"));
    let events = all_events(host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    let [(probe, outcome)] = failed.as_slice() else {
        panic!("one failed probe: {failed:#?}");
    };
    assert!(
        probe.sequence() < started_at,
        "the probe ran before the recovery"
    );
    assert_eq!(
        (value(outcome, "attempts"), value(outcome, "failure_code")),
        (json!(1), json!(CAPTURE_OPEN_FAILED))
    );
    assert_eq!(
        phases(&events)
            .iter()
            .filter(|(_, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
            ))
            .count(),
        1,
        "the held recovery is admitted, not suppressed again"
    );
    wait_for_ladder_end(host, POLICY_INSTANCE_ALIAS);
    assert!(host.fatal_error().expect("runtime health").is_none());
    replay.host.close().expect("close host");
    drop(replay.root);
}

/// T2: with no cool-down (stuck recovery off), `first_interval_ms` 60 000, `factor` 3 and
/// `max_interval_ms` 300 000: no probe a millisecond before the first interval; the first probe at
/// it and the second 180 000 after it fail, each recorded with its number and the next probe's
/// time; the third is 300 000 (the cap, not 540 000) after the second and passes: the instance is
/// available and, however long the clock runs, nothing probes it again.
#[test]
fn avail1_t2_an_unavailable_instance_is_probed_at_growing_intervals_up_to_the_cap() {
    const FIRST_MS: u64 = 60_000;
    const CAP_MS: u64 = 300_000;
    let probed = Probed::start(&probe_keys(FIRST_MS, 3, CAP_MS, 3_600_000));
    let host = &probed.host;
    let entered = unavailable(host).expect("the instance is unavailable after its failed check");
    assert_eq!(entered.probes, 0);
    assert_eq!(entered.last_probe_unix_ms, None);
    assert_eq!(
        entered.next_probe_unix_ms,
        Some(probed.now_unix_ms() + FIRST_MS),
        "the first probe is one first interval after it became unavailable"
    );
    probed.clock.advance(FIRST_MS - 1);
    probed.tick();
    assert_eq!(
        unavailable(host).expect("still unavailable").probes,
        0,
        "no probe before the first interval"
    );
    for (probe, interval_ms) in [(1_u32, 180_000_u64), (2, CAP_MS)] {
        probed.advance_to_next_probe();
        probed.tick();
        wait_until("the failed probe's record", || {
            records(host, HostCode::InstanceUnavailableProbeFailed) == probe as usize
        });
        let after = unavailable(host).expect("still unavailable");
        assert_eq!(after.probes, probe);
        let last = after.last_probe_unix_ms.expect("the probe's end");
        assert_eq!(
            after.next_probe_unix_ms,
            Some(last + interval_ms),
            "probe {probe}: the next interval is the one before times the factor, at most the cap"
        );
    }
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(unavailable(host), None, "the third probe made it available");
    assert_eq!(last_availability(host), Some(true));
    let events = all_events(host);
    let failed = failed_outcomes(&events, HostCode::InstanceUnavailableProbeFailed);
    assert_eq!(
        failed
            .iter()
            .map(|(event, outcome)| (
                event.severity(),
                value(outcome, "attempts"),
                value(outcome, "failure_code"),
                outcome.value("next_occurrence_unix_ms").is_some(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                EventSeverity::Warning,
                json!(1),
                json!(CAPTURE_OPEN_FAILED),
                true
            ),
            (
                EventSeverity::Warning,
                json!(2),
                json!(CAPTURE_OPEN_FAILED),
                true
            ),
        ]
    );
    let passed = failed_outcomes(&events, HostCode::InstanceUnavailableProbePassed);
    let [(passed_event, passed)] = passed.as_slice() else {
        panic!("one passing probe: {passed:#?}");
    };
    assert_eq!(passed_event.severity(), EventSeverity::Info);
    assert_eq!(value(passed, "attempts"), json!(3));
    let prepared = recovery_preparations(host);
    probed.clock.advance(CAP_MS * 10);
    probed.tick();
    probed.tick();
    assert_eq!(recovery_preparations(host), prepared, "no probe after it");
    assert_eq!(records(host, HostCode::InstanceUnavailableProbeFailed), 2);
    assert_eq!(records(host, HostCode::InstanceUnavailableProbePassed), 1);
    probed.close();
}

/// T3, the hold `hold` puts on the unavailable instance: however far past its first interval the
/// clock runs, no probe of it runs or is recorded.
fn assert_not_probed(probed: &Probed, hold: &str) {
    let host = &probed.host;
    probed.clock.advance(60_000 * 10);
    probed.tick();
    probed.tick();
    assert_eq!(
        (
            records(host, HostCode::InstanceUnavailableProbeFailed),
            records(host, HostCode::InstanceUnavailableProbePassed),
        ),
        (0, 0),
        "{hold}: no probe"
    );
}

/// T3, a paused instance: a person's pause (`stand`) holds it; it stays unavailable with no probe
/// and no self-check at all.
#[test]
fn avail1_t3_a_paused_instance_is_not_probed() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    let receipt = host
        .process_request_for_test(
            &instance_pause_as(POLICY_INSTANCE_ALIAS, SchedulingPauseMode::Stand),
            ConnectionId::new(4_892).expect("connection"),
        )
        .expect("pause receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let prepared = recovery_preparations(host);
    assert_not_probed(&probed, "paused");
    assert_eq!(recovery_preparations(host), prepared);
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "it stays unavailable, unprobed"
    );
    probed.close();
}

/// T3, a fenced instance: the fence takes it (its own recheck owns it), so it leaves the
/// unavailable instances (the status shows its fence, not `unavailable`) and no probe of it runs.
#[test]
fn avail1_t3_a_fenced_instance_is_not_probed() {
    let probed = Probed::start(&probe_keys(60_000, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    host.fence_instance_for_test(POLICY_INSTANCE_ALIAS)
        .expect("fence the instance");
    let status = instance_status(host);
    assert!(status.resource_fence().is_some(), "{status:?}");
    assert_eq!(status.unavailable(), None);
    assert_not_probed(&probed, "fenced");
    assert_eq!(unavailable(host), None);
    probed.close();
}

/// T3, an instance whose emulator the idle action holds stopped (`stop_emulator` done, its wake
/// not come; the configured action is `stop_emulator`, so no check wakes it): no probe and no
/// self-check of it runs.
#[test]
fn avail1_t3_an_idle_stopped_instance_is_not_probed() {
    let mut file = probe_keys(60_000, 2, 1_800_000, 3_600_000);
    file["idle"] = json!({"action": "stop_emulator"});
    let probed = Probed::start(&file);
    let host = &probed.host;
    host.hold_emulator_stopped_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the idle action holds the emulator stopped");
    let prepared = recovery_preparations(host);
    assert_not_probed(&probed, "idle-stopped");
    assert_eq!(recovery_preparations(host), prepared);
    assert_eq!(
        unavailable(host).map(|unavailable| unavailable.probes),
        Some(0),
        "it stays unavailable, unprobed"
    );
    probed.close();
}

/// T4: `attention_after_ms` 120 000 and a first interval of 600 000. A millisecond before the
/// attention time nothing is raised; at it one warning `instance_unavailable_awaiting_person` under
/// the instance, with how long, the configured time, the probes so far, the cause and the next
/// probe's time, and the status says `awaiting_person`. A failing probe after it raises nothing
/// more; the passing one ends the unavailability, and with it the attention: the status member is
/// gone, and still one record.
#[test]
fn avail1_t4_the_attention_is_raised_once_and_ends_with_the_unavailability() {
    const ATTENTION_MS: u64 = 120_000;
    const FIRST_MS: u64 = 600_000;
    let probed = Probed::start(&probe_keys(FIRST_MS, 2, 1_800_000, ATTENTION_MS));
    let host = &probed.host;
    let since = unavailable(host)
        .expect("the instance is unavailable")
        .since_unix_ms;
    probed
        .clock
        .advance((since + ATTENTION_MS - 1).saturating_sub(probed.now_unix_ms()));
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        0
    );
    assert!(!unavailable(host).expect("unavailable").awaiting_person);
    probed.clock.advance(1);
    probed.tick();
    wait_until("the attention's record", || {
        records(host, HostCode::InstanceUnavailableAwaitingPerson) == 1
    });
    let state = unavailable(host).expect("unavailable");
    assert!(state.awaiting_person, "{state:?}");
    let events = all_events(host);
    let raised = failed_outcomes(&events, HostCode::InstanceUnavailableAwaitingPerson);
    let [(event, outcome)] = raised.as_slice() else {
        panic!("one attention: {raised:#?}");
    };
    assert_eq!(event.severity(), EventSeverity::Warning);
    assert_eq!(event.links().instance_id(), Some(&probed.registered));
    assert_eq!(
        (
            value(outcome, "elapsed_ms"),
            value(outcome, "limit_ms"),
            value(outcome, "attempts"),
            value(outcome, "failure_code"),
            value(outcome, "next_occurrence_unix_ms"),
        ),
        (
            json!(ATTENTION_MS),
            json!(ATTENTION_MS),
            json!(0),
            json!(CAPTURE_OPEN_FAILED),
            json!(state.next_probe_unix_ms.expect("the next probe")),
        )
    );
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the failed probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        1,
        "not raised again at a probe"
    );
    assert!(unavailable(host).expect("unavailable").awaiting_person);
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    assert_eq!(
        unavailable(host),
        None,
        "the attention ends with the unavailability"
    );
    probed.tick();
    assert_eq!(
        records(host, HostCode::InstanceUnavailableAwaitingPerson),
        1
    );
    probed.close();
}

/// T5: before its self-check failed the instance's status has no `unavailable` member; after it,
/// the member says since when, why, the probes and the next one's time; after a failed probe, the
/// probe's end and the next time; and once a probe passed, the instance's status is, byte for byte,
/// what it was before. Each read is a second after the step before it, past every takeover
/// cool-down.
#[test]
fn avail1_t5_the_status_says_it_and_an_available_instance_is_unchanged() {
    const FIRST_MS: u64 = 60_000;
    let probed = Probed::started(&probe_keys(FIRST_MS, 2, 1_800_000, 3_600_000));
    let host = &probed.host;
    let since = probed.now_unix_ms();
    probed.clock.advance(1_000);
    let before = serde_json::to_vec(&instance_status(host)).expect("the status before");
    assert!(
        !String::from_utf8_lossy(&before).contains("\"unavailable\""),
        "{}",
        String::from_utf8_lossy(&before)
    );
    probed.fail_self_check();
    let failed_at = probed.now_unix_ms();
    let projected = serde_json::to_value(instance_status(host)).expect("the status");
    assert_eq!(
        projected["unavailable"],
        json!({
            "since_unix_ms": since,
            "cause": CAPTURE_OPEN_FAILED,
            "probes": 0,
            "next_probe_unix_ms": failed_at + FIRST_MS,
            "awaiting_person": false,
        }),
        "{projected}"
    );
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the failed probe's record", || {
        records(host, HostCode::InstanceUnavailableProbeFailed) == 1
    });
    let projected = serde_json::to_value(instance_status(host)).expect("the status");
    let last = projected["unavailable"]["last_probe_unix_ms"]
        .as_u64()
        .expect("the probe's end");
    assert_eq!(
        projected["unavailable"],
        json!({
            "since_unix_ms": since,
            "cause": CAPTURE_OPEN_FAILED,
            "probes": 1,
            "last_probe_unix_ms": last,
            "next_probe_unix_ms": last + 2 * FIRST_MS,
            "awaiting_person": false,
        }),
        "{projected}"
    );
    probed.passes();
    probed.advance_to_next_probe();
    probed.tick();
    wait_until("the passing probe's record", || {
        records(host, HostCode::InstanceUnavailableProbePassed) == 1
    });
    probed.clock.advance(1_000);
    let after = serde_json::to_vec(&instance_status(host)).expect("the status after");
    assert_eq!(
        String::from_utf8_lossy(&after),
        String::from_utf8_lossy(&before),
        "an available instance's status is what it was"
    );
    probed.close();
}
