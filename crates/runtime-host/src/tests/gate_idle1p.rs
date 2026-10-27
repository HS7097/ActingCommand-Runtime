// SPDX-License-Identifier: AGPL-3.0-only

//! #405 IDLE-1P (brief rd-1027/brief-idle1p.md; audit #420 section 2 item 4). Production on
//! 10-27: a game closed by IDLE-1 since 14:35 with its wake at 17:54 was forgotten by a restart at
//! 17:32 (the planner kept its state in memory only), and its 18:01 task met the closed game. The
//! required state:
//! - L-1: a closed instance with its wake pending, then a restart: the state is restored (the
//!   instance is not acted on again) and the wake fires at its time.
//! - L-2: a closed instance, then a restart within the wake lead of its next task: it is woken at
//!   the start's first check, before any plan.
//! - L-3: an instance whose state the start does not know (an earlier start kept none) is not
//!   treated as open: it is not closed, and it is woken at its next task minus the lead.
//! - L-5: a fact-refresh reading with no due task does not wake a closed instance (the scheduler
//!   says why: `fact_refresh_pending` with the gate `instance_idle_action`, `gate_d2_s4`).
//! - L-6: a reading that a task due within the lead needs wakes it.
//!
//! Review fixes (rd-1027/brief-idle1p-fix1.md): M-2, a record an older build outlived (a
//! rollback) leaves the instance closed; L-3, the daemon-start autostart leaves an emulator the
//! restored `stop_emulator` holds stopped; L-2, any eligible task with no later wake is due.
//!
//! L-1 to L-3 go through the host with the scripted emulator of `gate_idle1_host` (X's idle action
//! `stop_emulator`, its configured emulator start and a startup package) on a manual clock; each
//! restart starts a new host on the same state root. L-5 and L-6 read the plan the idle check
//! reads from a recorded evaluation (`ScheduleExplanationTable`) on a manual clock. Every wait
//! returns as soon as what it waits for has happened; `WAIT` only bounds a broken run.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{connectivity, head};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation};
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, WakeWork};
use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::{
    EmulatorInstanceAction, FactScalar, FactValue, InstanceStuckRecovery, RuntimeConfigManifest,
    RuntimeFactScope, RuntimeLifecyclePhase,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_policy::{
    EligibilityState, FACT_REFRESH_TASK_ID, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision, TaskRank,
};
use actingcommand_runtime_config::{
    DefaultsFile, IdleAction, IdleConfig, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);
const X_ALIAS: &str = "idle1p.instance.x";
const MINUTE_MS: u64 = 60_000;
/// The manual clock's Unix reading at the first start.
const NOW_UNIX_MS: u64 = 1_000;
/// X's next due task: four hours away, past `idle.min_gap_ms`'s default.
const NEXT_DUE_UNIX_MS: u64 = NOW_UNIX_MS + 4 * 60 * MINUTE_MS;
/// `idle.stop_emulator_wake_lead_ms`'s default.
const STOP_EMULATOR_LEAD_MS: u64 = 300_000;
/// X's wake: its next due task minus the lead.
const WAKE_UNIX_MS: u64 = NEXT_DUE_UNIX_MS - STOP_EMULATOR_LEAD_MS;
/// The kept state's key.
const IDLE_STATE: &str = "host.idle_state";

fn instance_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The configuration actingd assembles for X over the defaults file: X's entry names
/// `start_emulator` and the idle action `stop_emulator`.
fn idle_manifest(x: InstanceId) -> RuntimeConfigManifest {
    let id = instance_text(x);
    let file = json!({"instances": [{
        "instance_id": id,
        "start_emulator": true,
        "idle": {"action": "stop_emulator"}
    }]});
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        std::slice::from_ref(&id),
        &defaults,
        &file,
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// X's scripted device: emulator-controlled from the start, so it registers with its emulator
/// running.
fn x_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.emulator_control.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    state
}

/// A start of the Runtime on `root` with X alone (X's stuck recovery off, so no ladder acts on
/// it), on `clock`; `failing`, the start fails right after its start event.
fn start(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    x: InstanceId,
    state: &Arc<FakeState>,
    failing: bool,
) -> RuntimeHostResult<RuntimeHost> {
    start_with(root, clock, x, state, |config| {
        if failing {
            config.with_start_failure_after_start_event_for_test()
        } else {
            config
        }
    })
}

/// [`start`] with `adjust` applied to its configuration.
fn start_with(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    x: InstanceId,
    state: &Arc<FakeState>,
    adjust: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHostResult<RuntimeHost> {
    let package_path = root.path().join("idle1p-startup-package.zip");
    fs::write(&package_path, neutral_contained_task_package(true)).expect("write startup package");
    let startup = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("startup package request");
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let config = config(root)
        .with_runtime_clock(clock.clone())
        .with_stuck_recovery(BTreeMap::from([(
            X_ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: false,
                cooldown_secs: 600,
            },
        )]))
        .with_emulator_autostart(vec![X_ALIAS.to_owned()])
        .with_network_connectivity_for_test(read)
        .with_startup_packages(BTreeMap::from([(X_ALIAS.to_owned(), startup)]))
        .with_config_manifest(idle_manifest(x));
    RuntimeHost::start(
        adjust(config),
        Arc::new(FakeProvider::from_entries([(
            X_ALIAS.to_owned(),
            x,
            Arc::clone(state),
        )])),
    )
}

/// X's plan evaluated at `at`: no task due, the next one at `NEXT_DUE_UNIX_MS`.
fn x_plan_at(at: u64) -> BTreeMap<String, IdlePlan> {
    BTreeMap::from([(
        X_ALIAS.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: at,
            due_now: false,
            next_wake_unix_ms: Some(NEXT_DUE_UNIX_MS),
            reading_due_unix_ms: None,
        },
    )])
}

/// No plan at all: before the first evaluated cycle of a start.
fn no_plan() -> BTreeMap<String, IdlePlan> {
    BTreeMap::new()
}

/// X's idle records after `after`, in order: `instance_idle_action` with its operation, and the
/// wake's `emulator_autostart_queued`.
fn idle_records(host: &RuntimeHost, x: InstanceId, after: u64) -> Vec<String> {
    let events = all_events(host);
    phases(&events)
        .into_iter()
        .filter(|(event, _)| event.sequence() > after && event.links().instance_id() == Some(&x))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::DeviceStateObserved { outcome } => {
                let code = outcome.code().as_str();
                if code == IdleCode::Taken.as_str() {
                    let operation = outcome
                        .value("operation")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(format!("{code} {operation}"))
                } else if code == HostCode::EmulatorAutostartQueued.as_str() {
                    Some(code.to_owned())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect()
}

fn action_record() -> String {
    format!(
        "{} {}",
        IdleCode::Taken.as_str(),
        IdleLocation::Action.as_str()
    )
}

/// One wake of X after `stop_emulator`: its record, then its configured emulator start.
fn wake_records() -> Vec<String> {
    vec![
        format!(
            "{} {}",
            IdleCode::Taken.as_str(),
            IdleLocation::Wake.as_str()
        ),
        HostCode::EmulatorAutostartQueued.as_str().to_owned(),
    ]
}

/// The instance rows of `host.idle_state` the host holds, after its writer row; none when it
/// holds no such fact.
fn kept_rows(host: &RuntimeHost) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    kept_record(host).map(|(_, rows)| rows)
}

/// `host.idle_state` as the host holds it: the owner epoch its writer row names (review M-2),
/// and its instance rows.
fn kept_record(host: &RuntimeHost) -> Option<(String, Vec<BTreeMap<String, FactScalar>>)> {
    host.runtime_fact_snapshot()
        .expect("the program facts")
        .records
        .into_iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == IDLE_STATE)
        .map(|record| match record.value {
            FactValue::RecordList(mut rows) => {
                assert!(!rows.is_empty(), "host.idle_state has its writer row");
                let writer = rows.remove(0);
                let epoch = match (writer.len(), writer.get("owner_epoch")) {
                    (1, Some(FactScalar::String(epoch))) => epoch.clone(),
                    _ => panic!("the first row names the writer's owner_epoch alone: {writer:?}"),
                };
                (epoch, rows)
            }
            other => panic!("host.idle_state is a record list: {other:?}"),
        })
}

/// X's row: closed by `stop_emulator`, its wake `wake` when one is planned.
fn x_row(wake: Option<u64>) -> BTreeMap<String, FactScalar> {
    let mut row = BTreeMap::from([
        (
            "instance_alias".to_owned(),
            FactScalar::String(X_ALIAS.to_owned()),
        ),
        (
            "action".to_owned(),
            FactScalar::String("stop_emulator".to_owned()),
        ),
    ]);
    if let Some(at) = wake {
        row.insert("wake_at_unix_ms".to_owned(), FactScalar::TimestampMs(at));
    }
    row
}

fn stops(state: &FakeState) -> usize {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| *call == EmulatorCall::Control(EmulatorInstanceAction::Stop))
        .count()
}

/// The first start: X's action runs (an unscripted Stop is confirmed) for its next task at
/// `NEXT_DUE_UNIX_MS`; the host keeps X closed with its wake. Returns the host and the ledger head
/// before the action.
fn close_x(root: &TempDir, clock: &Arc<ManualRuntimeClock>, x: InstanceId) -> (RuntimeHost, u64) {
    let state = x_state();
    let host = start(root, clock, x, &state, false).expect("the first start");
    let before = head(&host);
    host.check_idle_instances_for_test(&x_plan_at(NOW_UNIX_MS))
        .expect("the idle check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, WAIT),
        Some(1),
        "X's idle action ended"
    );
    assert_eq!(stops(&state), 1, "X's action asked for its Stop");
    assert_eq!(idle_records(&host, x, before), [action_record()]);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X is kept closed with its wake"
    );
    (host, before)
}

#[test]
fn idle1p_l1_a_restart_restores_the_closed_instance_and_its_wake_fires() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let (first, _) = close_x(&root, &clock, x);
    first.close().expect("close the first host");

    // The restart, well before the wake.
    let restarted_at = NOW_UNIX_MS + 30 * MINUTE_MS;
    clock.set_unix_ms(restarted_at);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the restart");
    let after_start = head(&host);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "the restart restores X closed with its wake and keeps it again"
    );

    // Checks before the wake, without a plan and with a fresh one: X is neither acted on again
    // nor woken.
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check before any plan");
    host.check_idle_instances_for_test(&x_plan_at(restarted_at))
        .expect("the check with a plan");
    assert_eq!(stops(&state), 0, "the restored X is not stopped again");
    assert!(
        idle_records(&host, x, after_start).is_empty(),
        "no action and no wake before the wake's time"
    );

    // The wake's time, still without a plan: the restored wake fires.
    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&no_plan())
        .expect("the check at the wake");
    assert_eq!(idle_records(&host, x, after_start), wake_records());
    assert_eq!(
        kept_rows(&host),
        Some(Vec::new()),
        "X is no longer kept closed"
    );
    host.close().expect("close the restarted host");
}

#[test]
fn idle1p_l2_a_restart_within_the_wake_lead_wakes_the_instance_at_start() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let (first, before) = close_x(&root, &clock, x);
    first.close().expect("close the first host");

    // The restart inside the lead of X's next task (as 17:32 was to the 17:54 wake of 10-27).
    clock.set_unix_ms(NEXT_DUE_UNIX_MS - STOP_EMULATOR_LEAD_MS / 2);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the restart");
    // The start's first check, before any evaluated cycle (the lease sweep's may be the one that
    // takes it): the restored wake has come.
    host.check_idle_instances_for_test(&no_plan())
        .expect("the start's check");
    let mut expected = vec![action_record()];
    expected.extend(wake_records());
    assert_eq!(
        idle_records(&host, x, before),
        expected,
        "one action before the restart, one wake at the start"
    );
    assert_eq!(stops(&state), 0, "X is not stopped again");
    host.close().expect("close the restarted host");
}

#[test]
fn idle1p_l3_an_unknown_state_at_start_is_not_treated_as_open() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    // An earlier start that kept no idle state (it fails right after its start event).
    let Err(_) = start(&root, &clock, x, &x_state(), true) else {
        panic!("the earlier start did not fail after its start event");
    };

    clock.set_unix_ms(NOW_UNIX_MS + MINUTE_MS);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the start");
    let after_start = head(&host);
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(None)]),
        "X, whose state the start does not know, counts as closed"
    );
    // A plan with no task within the minimum gap would close an open X: X is not acted on.
    let now = NOW_UNIX_MS + 2 * MINUTE_MS;
    clock.set_unix_ms(now);
    host.check_idle_instances_for_test(&x_plan_at(now))
        .expect("the check");
    assert_eq!(stops(&state), 0, "X is not treated as open");
    assert!(idle_records(&host, x, after_start).is_empty());

    // Its wake comes at its next task minus the lead.
    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&x_plan_at(WAKE_UNIX_MS))
        .expect("the check at the wake");
    assert_eq!(idle_records(&host, x, after_start), wake_records());
    host.close().expect("close the host");
}

/// Review M-2 (a rollback): v0.12.20 keeps its state, an older build runs (it keeps none, and its
/// own idle action closes X), then v0.12.20 starts again. The record names the first start, not
/// the owner before this one, so it is stale: X's state is unknown and X counts as closed, never
/// as open: it is not acted on, and it is woken at its next task minus the lead.
#[test]
fn idle1p_m2_a_record_an_older_build_outlived_leaves_the_instance_closed() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    // v0.12.20: the ledger's first start keeps X open (nothing closed) under its own epoch.
    let first = start(&root, &clock, x, &x_state(), false).expect("the first start");
    let (first_epoch, rows) = kept_record(&first).expect("the first start keeps its state");
    assert!(
        rows.is_empty(),
        "nothing is closed at the ledger's first start"
    );
    first.close().expect("close the first host");

    // The older build: it keeps no idle state, and its idle action closes X.
    let older_at = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(older_at);
    let older_state = x_state();
    let older = start_with(
        &root,
        &clock,
        x,
        &older_state,
        RuntimeHostConfig::with_idle_state_unkept_for_test,
    )
    .expect("the older build's start");
    older
        .check_idle_instances_for_test(&x_plan_at(older_at))
        .expect("the older build's check");
    assert_eq!(
        older.await_idle_action_ends_for_test(1, WAIT),
        Some(1),
        "the older build's idle action ended"
    );
    assert_eq!(stops(&older_state), 1, "the older build stopped X");
    older.close().expect("close the older build");

    // v0.12.20 again.
    let now = NOW_UNIX_MS + 2 * MINUTE_MS;
    clock.set_unix_ms(now);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the start after the rollback");
    let after_start = head(&host);
    let (epoch, rows) = kept_record(&host).expect("the start keeps its state");
    assert_ne!(
        epoch, first_epoch,
        "the start keeps its state under its own epoch"
    );
    assert_eq!(
        rows,
        vec![x_row(None)],
        "X, whose record is stale, counts as closed"
    );
    host.check_idle_instances_for_test(&x_plan_at(now))
        .expect("the check");
    assert_eq!(stops(&state), 0, "X is not treated as open");
    assert!(idle_records(&host, x, after_start).is_empty());

    clock.set_unix_ms(WAKE_UNIX_MS);
    host.check_idle_instances_for_test(&x_plan_at(WAKE_UNIX_MS))
        .expect("the check at the wake");
    assert_eq!(idle_records(&host, x, after_start), wake_records());
    host.close().expect("close the host");
}

/// Review L-3: the daemon-start emulator start (S6c) leaves an emulator that the restored
/// `stop_emulator` holds stopped to its wake: a restart does not bring a closed instance back.
#[test]
fn idle1p_l3_the_daemon_start_autostart_leaves_a_held_stopped_emulator_to_its_wake() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let (first, _) = close_x(&root, &clock, x);
    first.close().expect("close the first host");

    // The restart, well before the wake, with X's emulator stopped as the action left it.
    clock.set_unix_ms(NOW_UNIX_MS + 30 * MINUTE_MS);
    let state = x_state();
    state.emulator.rebind(None);
    state.refuse_pending_opens.store(true, Ordering::Release);
    let host = start(&root, &clock, x, &state, false).expect("the restart");
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(WAKE_UNIX_MS))]),
        "X is restored closed with its wake"
    );
    assert_eq!(
        host.enqueue_emulator_autostart()
            .expect("the daemon-start autostart"),
        0,
        "X's held-stopped emulator gets no autostart claim"
    );
    assert!(
        !state
            .emulator
            .calls()
            .contains(&EmulatorCall::Control(EmulatorInstanceAction::Start)),
        "X's emulator is not started before its wake"
    );
    host.close().expect("close the restarted host");
}

// ---- L-5, L-6: the plan the idle check reads, from a recorded evaluation ----

const NODE: &str = "node-a";
const T0: u64 = 1_800_000_000_000;
/// The production keys of 10-26: `close_app`, its wake lead 300000, `idle.min_gap_ms` 1800000.
const LEAD: u64 = 5 * MINUTE_MS;
const PRODUCTION: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 30 * MINUTE_MS,
    close_app_wake_lead_ms: LEAD,
    stop_emulator_wake_lead_ms: 5 * MINUTE_MS,
};
const WAKE: IdleStep = IdleStep::Wake {
    action: IdleAction::CloseApp,
    work: &[WakeWork::StartupPackage],
};

fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// One decision of `task_id` on the instance.
fn decision(
    task_id: &str,
    eligibility: EligibilityState,
    state: SchedulingDecisionState,
    next_wake_unix_ms: Option<u64>,
) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility,
        state,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: Vec::new(),
        next_wake_unix_ms,
    }
}

/// The idle check's plan for the instance from an evaluation of `decisions` at `at`.
fn explained(decisions: Vec<TaskDecision>, at: u64) -> Option<IdlePlan> {
    let mut table = ScheduleExplanationTable::default();
    table.record(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:idle1p",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
    );
    table.idle_plan(NODE)
}

/// The instance's task four hours away: its trigger is false until then.
fn later_task(next_due: u64) -> TaskDecision {
    decision(
        "task.later",
        EligibilityState::False,
        SchedulingDecisionState::Blocked,
        Some(next_due),
    )
}

/// `close_app` on the instance at `T0` for its next task at `next_due`, done.
fn closed(next_due: u64) -> IdlePlanner {
    let mut planner = IdlePlanner::default();
    let plan = explained(vec![later_task(next_due)], T0);
    assert_eq!(
        planner.check(NODE, &PRODUCTION, free(), plan, T0),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(next_due),
            wake_at_unix_ms: Some(next_due - LEAD),
        })
    );
    planner.finished(NODE, IdleEnd::Done);
    planner
}

#[test]
fn idle1p_l5_a_reading_with_no_due_task_does_not_wake_the_closed_instance() {
    let next_due = T0 + 4 * 60 * MINUTE_MS;
    let now = T0 + 10 * MINUTE_MS;
    // The task that reads the fact waits for the reading with its trigger undecided: it is not
    // due. The reading and that task as the evaluator gives them (`gate_d2_s4`):
    // 1. the reading selected (an evaluation that did not know the instance closed): the task
    //    waits under `fact_refresh_pending` with no wake (§3.7 row 3);
    // 2. the reading held back by the idle action, past its own gates (no wake): the task waits
    //    with the gate `instance_idle_action` and no wake (`idle1p_l5_a_reading_...`);
    // 3. the reading held by its own minimum interval or failure backoff on the closed instance
    //    (review M-1): the reading keeps its own wake, the cooldown's end, and the task waits with
    //    the gate `task_cooldown_active` and takes no wake of the reading's
    //    (`idle1p_l5_within_the_cooldown_...`).
    let waiting = || {
        decision(
            "task.reads_the_fact",
            EligibilityState::Unknown,
            SchedulingDecisionState::Deferred,
            None,
        )
    };
    for (reading, waiting) in [
        (
            decision(
                FACT_REFRESH_TASK_ID,
                EligibilityState::True,
                SchedulingDecisionState::Selected,
                None,
            ),
            waiting(),
        ),
        (
            decision(
                FACT_REFRESH_TASK_ID,
                EligibilityState::True,
                SchedulingDecisionState::Deferred,
                None,
            ),
            waiting(),
        ),
        (
            decision(
                FACT_REFRESH_TASK_ID,
                EligibilityState::True,
                SchedulingDecisionState::Deferred,
                Some(now + 5 * MINUTE_MS),
            ),
            waiting(),
        ),
    ] {
        let mut planner = closed(next_due);
        let plan = explained(vec![reading.clone(), waiting, later_task(next_due)], now);
        let read = plan.expect("an evaluated plan");
        assert!(!read.due_now, "{reading:?}: the reading is no due task");
        assert_eq!(read.next_wake_unix_ms, Some(next_due), "{reading:?}");
        assert!(read.reading_due_unix_ms.is_some(), "{reading:?}");
        assert_eq!(
            planner.check(NODE, &PRODUCTION, free(), plan, now),
            None,
            "{reading:?}: the reading alone does not wake the closed instance"
        );
        assert_eq!(
            planner.planned_wake(NODE),
            Some(next_due - LEAD),
            "{reading:?}: nor does it move the wake"
        );
        // The natural wake: the next task minus the lead.
        let wake = next_due - LEAD;
        let plan = explained(vec![reading.clone(), later_task(next_due)], wake);
        assert_eq!(
            planner.check(NODE, &PRODUCTION, free(), plan, wake),
            Some(WAKE),
            "{reading:?}"
        );
    }
}

#[test]
fn idle1p_l6_a_reading_a_task_due_within_the_lead_needs_wakes_the_closed_instance() {
    let next_due = T0 + 4 * 60 * MINUTE_MS;
    let now = T0 + 10 * MINUTE_MS;
    let reading = || {
        decision(
            FACT_REFRESH_TASK_ID,
            EligibilityState::True,
            SchedulingDecisionState::Selected,
            None,
        )
    };
    // The task that needs the reading is due now (its trigger true, waiting for the reading),
    // or due within the lead (deferred to a wake inside it).
    for needing in [
        decision(
            "task.needs_the_fact",
            EligibilityState::True,
            SchedulingDecisionState::Deferred,
            None,
        ),
        decision(
            "task.needs_the_fact",
            EligibilityState::True,
            SchedulingDecisionState::Deferred,
            Some(now + LEAD / 2),
        ),
    ] {
        let mut planner = closed(next_due);
        let plan = explained(vec![reading(), needing.clone(), later_task(next_due)], now);
        assert_eq!(
            planner.check(NODE, &PRODUCTION, free(), plan, now),
            Some(WAKE),
            "{needing:?}"
        );
    }
}

/// Review L-2 (IDLE-1P keeps the new meaning): a task of the instance is due when any eligible
/// decision of it has no wake of its own later than the evaluation, not only the top-ranked one.
/// Here the top-ranked eligible task is deferred to a later wake (a random delay) and a
/// lower-ranked one waits with none: the plan is due now, so an open instance is not closed and a
/// closed one is woken.
#[test]
fn idle1p_l2p_any_eligible_task_with_no_later_wake_is_due() {
    let next_due = T0 + 4 * 60 * MINUTE_MS;
    let now = T0 + 10 * MINUTE_MS;
    let ranked = |task_id: &str, total_score: i64, wake: Option<u64>| TaskDecision {
        rank: Some(TaskRank {
            priority: 0,
            aging_ms: 0,
            strategic_weight_milli: 1_000,
            urgency_milli: 0,
            load_cost_milli: 0,
            contention_penalty: 0,
            total_score,
            effective_milli: 0,
        }),
        ..decision(
            task_id,
            EligibilityState::True,
            SchedulingDecisionState::Deferred,
            wake,
        )
    };
    let decisions = || {
        vec![
            ranked("task.top", 900, Some(now + 60 * MINUTE_MS)),
            ranked("task.lower", 100, None),
            later_task(next_due),
        ]
    };
    let plan = explained(decisions(), now);
    assert!(
        plan.is_some_and(|plan| plan.due_now),
        "the lower-ranked task with no later wake is due: {plan:?}"
    );
    let mut open = IdlePlanner::default();
    assert_eq!(
        open.check(NODE, &PRODUCTION, free(), plan, now),
        None,
        "the open instance is not closed"
    );
    let mut planner = closed(next_due);
    assert_eq!(
        planner.check(NODE, &PRODUCTION, free(), explained(decisions(), now), now),
        Some(WAKE),
        "the closed instance is woken"
    );
}
