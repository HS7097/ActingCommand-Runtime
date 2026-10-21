// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A38 (0.12.9; the 10-20 incident: one instance's capture channel could not open for a
//! moment, its fence's ladder could not restart the emulator because the shutdown command timed
//! out, and the instance waited two hours for a person although one self-check would have passed
//! at once). Requirements of the slice (coordinator, 10-20 21:4x):
//!
//! - T1: a fence that waits for a person runs the instance's self-check every configured
//!   interval; two fail and the instance stays fenced and waiting, each recorded with its
//!   number; the third passes and lifts the fence with a record that carries its number, and
//!   the routine work the fence held is dispatched again;
//! - T2: the interval and the number of rechecks come from `recovery_ladder.fence_recheck`;
//!   nothing runs before the interval, and after the last recheck the instance keeps waiting
//!   and no further self-check runs;
//! - T3: a ladder's emulator shutdown that times out, while the one status read after it shows
//!   the emulator stopped, counts as a stop: the rung goes on and succeeds, with the timeout and
//!   the read recorded;
//! - T4: `Status` (what `actingctl status` prints and `ac_overview` shows as the row's
//!   `resource_fence`) shows the wait for a person: the cause, since when, the rechecks made and
//!   allowed and the next one's time; entering the wait is recorded.
//!
//! The fence of T1, T2 and T4 is a person's direct run whose input ends with an unconfirmed
//! close (a fence that manual work met waits for a person at once, Workflow #403 / M387 B1), on
//! a manual Runtime clock that only the test moves; each sweep tick is run by the test, so
//! nothing waits for time. Existing tests missed this because every #403 test stopped at
//! "the instance waits for a person" (gate_403, gate_411_a47 T2), and no emulator-control test
//! scripted a control command that passed its bound while the emulator did reach the state.

use super::emulator_controller::{ScriptedControl, stopped_state};
use super::gate_387_d1c::ladder_manifest;
use super::gate_403::status_fence;
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_setup, phases, rung_outcomes, wait_for_ladder_end,
    wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EmulatorInstanceAction, InstanceFencePersonWait, RecoveryRung, RecoveryRungOutcome,
    RecoveryTriggerStage, RuntimeLifecyclePhase, RuntimePayload, StartupPackageDisposition,
};
use actingcommand_device::DeviceProcessPresence;
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_device::codes::DeviceStage;
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const FENCED: &str = "fence.recheck.instance";

/// A host whose instance `FENCED` waits for a person: a person's direct run on it ended with an
/// unconfirmed close. `recovery_ladder.fence_recheck` is `{interval_ms, max_attempts}`; with
/// `startup`, the instance has the run's package as its startup package.
struct WaitingFence {
    root: TempDir,
    host: RuntimeHost,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    instance: InstanceId,
}

fn waiting_fence(interval_ms: u64, max_attempts: u32, startup: bool) -> WaitingFence {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = super::gate_369s5::direct_run_package(&root, &state);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let instance = instance_id();
    let mut host_config = config(&root)
        .with_runtime_clock(clock.clone())
        .with_config_manifest(ladder_manifest(json!({
            "fence_recheck": {"interval_ms": interval_ms, "max_attempts": max_attempts}
        })));
    if startup {
        host_config = host_config.with_startup_packages(BTreeMap::from([(
            FENCED.to_owned(),
            ContainedTaskRequest::new(package.clone()).expect("startup package"),
        )]));
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(FENCED, instance, Arc::clone(&state))),
    )
    .expect("runtime host");
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
        .process_request_for_test(&run, ConnectionId::new(380).expect("connection"))
        .expect("run receipt");
    assert_eq!(
        failed.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{failed:?}"
    );
    assert!(
        status_fence(&host, FENCED).is_some_and(|fence| fence.awaiting_person),
        "the instance waits for a person"
    );
    WaitingFence {
        root,
        host,
        state,
        clock,
        instance,
    }
}

/// The instance's wait for a person, as `Status` reports it.
fn person_wait(host: &RuntimeHost) -> InstanceFencePersonWait {
    status_fence(host, FENCED)
        .and_then(|fence| fence.person_wait)
        .expect("the instance waits for a person, with its rechecks")
}

fn now_unix_ms(clock: &ManualRuntimeClock) -> u64 {
    clock.sample().expect("manual clock").unix_ms
}

/// Moves the clock to `before_ms` before the instance's next recheck is due.
fn advance_to_recheck(host: &RuntimeHost, clock: &ManualRuntimeClock, before_ms: u64) {
    let next = person_wait(host)
        .next_recheck_unix_ms
        .expect("a next recheck");
    clock.advance(
        next.saturating_sub(now_unix_ms(clock))
            .saturating_sub(before_ms),
    );
}

/// The outcomes of the `runtime.failed` records whose code is `code`, in ledger order.
fn failed_outcomes<'a>(events: &'a [PersistedEvent], code: HostCode) -> Vec<&'a Outcome> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == code.as_str() =>
            {
                Some(record.outcome())
            }
            _ => None,
        })
        .collect()
}

fn recheck_failures(host: &RuntimeHost) -> usize {
    failed_outcomes(
        &all_events(host),
        HostCode::InstanceResourcesFenceRecheckFailed,
    )
    .len()
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

/// T1: two rechecks fail, the third passes. Each due recheck starts at the sweep tick after its
/// interval; a failed one leaves the instance fenced and waiting and the startup claim queued
/// behind the fence, and is recorded with its number; the passing one lifts the fence, recorded
/// as `resource_fence_lifted` with `instance_resources_fence_recheck_passed` and its number, the
/// proving preparation being the recheck's own; the startup claim the fence held then gets the
/// instance's key.
#[test]
fn a38_t1_a_waiting_fence_rechecks_itself_and_the_passing_recheck_lifts_it() {
    let fence = waiting_fence(60_000, 3, true);
    let host = &fence.host;
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
    let startup_request = all_events(host)
        .iter()
        .rev()
        .find(|event| event.event_type() == EventType::SchedulerQueued)
        .and_then(|event| event.links().request_id().copied())
        .expect("the startup claim's queueing");

    // The fake's opens report no passed native open: the first two self-checks fail.
    for attempt in 1..=2_u32 {
        advance_to_recheck(host, &fence.clock, 0);
        host.fence_recheck_tick_for_test().expect("sweep tick");
        assert_eq!(
            person_wait(host).recheck_attempts,
            attempt,
            "the due recheck {attempt} started"
        );
        wait_until("the failed recheck's record", || {
            recheck_failures(host) == attempt as usize
        });
        assert!(
            status_fence(host, FENCED).is_some_and(|fence| fence.awaiting_person),
            "a failed recheck leaves the instance fenced and waiting"
        );
        assert!(
            host.instance_claims_for_test(FENCED)
                .expect("claims")
                .1
                .contains(&ClaimKind::StartupPackage),
            "the startup claim still waits behind the fence"
        );
    }
    fence.state.observed_open.store(true, Ordering::Release);
    advance_to_recheck(host, &fence.clock, 0);
    host.fence_recheck_tick_for_test().expect("sweep tick");
    wait_until("the fence's lift record", || {
        phases(&all_events(host))
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::ResourceFenceLifted { .. }))
    });
    assert_eq!(status_fence(host, FENCED), None, "the fence is lifted");

    let events = all_events(host);
    let failures = failed_outcomes(&events, HostCode::InstanceResourcesFenceRecheckFailed);
    assert_eq!(
        failures
            .iter()
            .map(|outcome| (value(outcome, "attempts"), value(outcome, "limit_count")))
            .collect::<Vec<_>>(),
        vec![(json!(1), json!(3)), (json!(2), json!(3))],
        "each failed recheck is recorded with its number"
    );
    let lifts = phases(&events)
        .into_iter()
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::ResourceFenceLifted {
                instance_id,
                preparation,
                outcome,
            } => Some((event, instance_id, preparation, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(lifted, lifted_instance, preparation, outcome)] = lifts.as_slice() else {
        panic!("one lift record: {lifts:#?}");
    };
    assert_eq!(*lifted_instance, fence.instance);
    assert_eq!(
        outcome.code().as_str(),
        HostCode::InstanceResourcesFenceRecheckPassed.as_str()
    );
    assert_eq!(value(outcome, "attempts"), json!(3));
    assert_eq!(value(outcome, "limit_count"), json!(3));
    let proof = events
        .iter()
        .find(|event| event.sequence() == preparation.sequence)
        .expect("the proving preparation");
    assert!(matches!(
        phases(std::slice::from_ref(proof))
            .first()
            .map(|(_, phase)| phase.clone()),
        Some(RuntimeLifecyclePhase::InstancePreparationFinished {
            stage: RecoveryTriggerStage::RecoveryPreparation,
            outcome: None,
            ..
        })
    ));
    let lifted_at = lifted.sequence();
    wait_until(
        "the startup claim the fence held is granted the key",
        || {
            all_events(host).iter().any(|event| {
                matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                ) && event.links().request_id() == Some(&startup_request)
                    && event.sequence() > lifted_at
            })
        },
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    fence.host.close().expect("close host");
    drop(fence.root);
}

/// T2: with `interval_ms` 45 000 and `max_attempts` 2, the wait reports both; no recheck starts
/// a millisecond before the interval, each starts once it has passed (from the start of the wait,
/// then from the end of the failed recheck before), and after the second one the instance keeps
/// waiting with no next recheck: however long the clock runs, no further self-check runs.
#[test]
fn a38_t2_the_configured_attempts_run_at_the_configured_interval_then_no_more() {
    const INTERVAL_MS: u64 = 45_000;
    const MAX_ATTEMPTS: u32 = 2;
    let fence = waiting_fence(INTERVAL_MS, MAX_ATTEMPTS, false);
    let host = &fence.host;
    let wait = person_wait(host);
    assert_eq!(wait.recheck_max_attempts, MAX_ATTEMPTS);
    assert_eq!(wait.recheck_attempts, 0);
    assert_eq!(
        wait.next_recheck_unix_ms,
        Some(wait.since_unix_ms + INTERVAL_MS),
        "the first recheck is one interval after the wait began"
    );

    advance_to_recheck(host, &fence.clock, 1);
    host.fence_recheck_tick_for_test().expect("sweep tick");
    assert_eq!(
        person_wait(host).recheck_attempts,
        0,
        "no recheck before the interval"
    );
    for attempt in 1..=MAX_ATTEMPTS {
        advance_to_recheck(host, &fence.clock, 0);
        host.fence_recheck_tick_for_test().expect("sweep tick");
        assert_eq!(person_wait(host).recheck_attempts, attempt);
        wait_until("the failed recheck's record", || {
            recheck_failures(host) == attempt as usize
        });
        let ended = now_unix_ms(&fence.clock);
        assert_eq!(
            person_wait(host).next_recheck_unix_ms,
            (attempt < MAX_ATTEMPTS).then_some(ended + INTERVAL_MS),
            "the next recheck is one interval after a failed one, none after the last"
        );
    }
    let preparations = |host: &RuntimeHost| {
        phases(&all_events(host))
            .iter()
            .filter(|(_, phase)| {
                matches!(
                    phase,
                    RuntimeLifecyclePhase::InstancePreparationFinished { .. }
                )
            })
            .count()
    };
    let prepared = preparations(host);
    fence.clock.advance(INTERVAL_MS * 10);
    host.fence_recheck_tick_for_test().expect("sweep tick");
    host.fence_recheck_tick_for_test().expect("sweep tick");
    let wait = person_wait(host);
    assert_eq!(
        (wait.recheck_attempts, wait.next_recheck_unix_ms),
        (MAX_ATTEMPTS, None),
        "no recheck after the last one"
    );
    assert_eq!(preparations(host), prepared, "no further self-check ran");
    assert_eq!(recheck_failures(host), MAX_ATTEMPTS as usize);
    assert!(
        status_fence(host, FENCED).is_some_and(|fence| fence.awaiting_person),
        "the instance keeps waiting for a person"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    fence.host.close().expect("close host");
    drop(fence.root);
}

/// T3: a scheduled run fails and its ladder climbs from the emulator rung (the configured
/// `trigger_defaults.start_rung` 3; no startup package, so a ready environment ends the rung).
/// The rung's Stop control command passes its own bound (`mumu_manager_timeout`, no exit), and
/// the one status read after it shows the emulator stopped. The Stop counts as done: the rung
/// starts the emulator again, its readiness passes and the rung succeeds
/// (`environment_ready`); the timeout and the read are recorded as
/// `emulator_control_timeout_settled`, caused by the command's timeout.
#[test]
fn a38_t3_a_timed_out_emulator_stop_whose_status_read_shows_it_stopped_lets_the_rung_succeed() {
    let root = TempDir::new().expect("tempdir");
    let (host_config, request) = ladder_setup(root.path());
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let instance = instance_id();
    let host = RuntimeHost::start(
        host_config
            .with_startup_packages(BTreeMap::new())
            .with_config_manifest(ladder_manifest(json!({
                "trigger_defaults": {"start_rung": 3}
            }))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::clone(&state),
        )),
    )
    .expect("physical runtime host");
    bind_fake_emulator(&host, POLICY_INSTANCE_ALIAS, instance, &state);
    let context = admit_ladder_run(&host, &request);
    // The rung's readiness preparation passes its self-check.
    state.observed_open.store(true, Ordering::Release);
    state.emulator.script_control(
        EmulatorInstanceAction::Stop,
        ScriptedControl::Failed {
            stage: DeviceStage::MumuManagerTimeout.location(),
            exit_code: None,
            last_state: None,
            device_process_present: Some(DeviceProcessPresence::Unknown),
        },
    );
    state.emulator.script_read(stopped_state());
    state.unknown_capture.store(true, Ordering::Release);
    let failed = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!failed.is_fatal(), "{}", failed.code());
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let events = all_events(&host);
    let emulator_rung = rung_outcomes(&events)
        .into_iter()
        .find(|(rung, _, _)| *rung == RecoveryRung::EmulatorRestart);
    assert_eq!(
        emulator_rung,
        Some((
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::EnvironmentReady,
            None
        )),
        "the timed-out Stop that the status read shows stopped lets the rung succeed"
    );
    assert_eq!(
        state
            .emulator_actions
            .lock()
            .expect("fake emulator actions")
            .clone(),
        [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start]
    );
    let settled = failed_outcomes(&events, HostCode::EmulatorControlTimeoutSettled);
    let [settled] = settled.as_slice() else {
        panic!("one record of the settled timeout: {settled:#?}");
    };
    assert_eq!(
        (
            value(settled, "process_started"),
            value(settled, "android_started"),
            value(settled, "adb_port_present"),
        ),
        (json!(false), json!(false), json!(false)),
        "the status read is recorded"
    );
    assert!(
        settled
            .codes()
            .iter()
            .any(|code| code.as_str() == HostCode::EmulatorControlFailed.as_str()),
        "the command's timeout is recorded with it: {settled:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
    drop(root);
}

/// T4: `Status` shows the wait: the fence's cause and since when, and its `person_wait` with
/// since when it waits, the rechecks made (0, then 1 after one failed) and allowed (3) and the
/// next one's time (one interval after the wait began, then after the failed recheck). Entering
/// the wait is recorded once with `instance_resources_fence_awaiting_person`.
#[test]
fn a38_t4_status_shows_the_wait_for_a_person_with_cause_since_attempts_and_next_recheck() {
    const INTERVAL_MS: u64 = 30_000;
    let fence = waiting_fence(INTERVAL_MS, 3, false);
    let host = &fence.host;
    let projected = serde_json::to_value(status_fence(host, FENCED).expect("the fence"))
        .expect("the status member");
    let since = projected["since_unix_ms"].as_u64().expect("since_unix_ms");
    assert_eq!(projected["awaiting_person"], json!(true));
    assert_eq!(
        projected["cause"],
        json!(SharedCode::InputBackendOperationFailed.as_str())
    );
    assert_eq!(
        projected["person_wait"],
        json!({
            "since_unix_ms": since,
            "recheck_attempts": 0,
            "recheck_max_attempts": 3,
            "next_recheck_unix_ms": since + INTERVAL_MS,
        }),
        "{projected}"
    );
    let events = all_events(host);
    let entered = failed_outcomes(&events, HostCode::InstanceResourcesFenceAwaitingPerson);
    let [entered] = entered.as_slice() else {
        panic!("one record of the wait's start: {entered:#?}");
    };
    assert_eq!(
        (
            value(entered, "failure_code"),
            value(entered, "limit_count"),
            value(entered, "retry_after_ms"),
            value(entered, "next_occurrence_unix_ms"),
        ),
        (
            json!(SharedCode::InputBackendOperationFailed.as_str()),
            json!(3),
            json!(INTERVAL_MS),
            json!(since + INTERVAL_MS),
        )
    );

    advance_to_recheck(host, &fence.clock, 0);
    host.fence_recheck_tick_for_test().expect("sweep tick");
    wait_until("the failed recheck's record", || {
        recheck_failures(host) == 1
    });
    let ended = now_unix_ms(&fence.clock);
    let projected = serde_json::to_value(status_fence(host, FENCED).expect("the fence"))
        .expect("the status member");
    assert_eq!(
        projected["person_wait"],
        json!({
            "since_unix_ms": since,
            "recheck_attempts": 1,
            "recheck_max_attempts": 3,
            "next_recheck_unix_ms": ended + INTERVAL_MS,
        }),
        "{projected}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    fence.host.close().expect("close host");
    drop(fence.root);
}
