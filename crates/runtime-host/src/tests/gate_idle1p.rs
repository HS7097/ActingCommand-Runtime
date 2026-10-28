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
//!   treated as open: it is not closed; since review N-1 (round 2) it owes its wake at the start
//!   and is woken once at the start's first check.
//! - L-5: a fact-refresh reading with no due task does not wake a closed instance (the scheduler
//!   says why: `fact_refresh_pending` with the gate `instance_idle_action`, `gate_d2_s4`).
//! - L-6: a reading that a task due within the lead needs wakes it.
//!
//! Review fixes (rd-1027/brief-idle1p-fix1.md): M-2, a record an older build outlived (a
//! rollback) leaves the instance closed; L-3, the daemon-start autostart leaves an emulator the
//! restored `stop_emulator` holds stopped; L-2, any eligible task with no later wake is due.
//!
//! Review fixes, round 2 (rd-1027/brief-idle1p-fix2.md): N-1, a start without a usable record
//! (none, another owner's, unreadable) restores each closed instance with its wake at the start:
//! one whose only work waits for an expired status reading is woken once at the first check and
//! closed again by the next plan with its proper wake, and its startup claim waits behind a pause
//! or the install hold; L-3 and M-2 now expect that wake at start. N-3, both failures of an
//! abandoned action (its key's release and its record) are reported.
//!
//! 0.12.25 (brief rd-1028/brief-wave1.md W1-2; #405 comments 6445765189 and 6444911128):
//! - O-2: a wake that falls due while a pause or a deploy's hold holds the instance only queues its
//!   startup package in memory, so the record keeps the instance closed and woken (`woken`) until
//!   its wake's claims have ended; a restart meanwhile wakes it again and the startup package runs
//!   after the restart. Adapted for it (the approved change: X stays kept closed after its wake
//!   until its wake's claims have ended): L-1, L-3, M-2 and N-1 both (the history's first two
//!   records, closed then woken; review L-1: in L-1, L-3 and M-2 the wake's start reads no status,
//!   so what follows pins a failed start, not a startup package that ran).
//! - O-2, review M-1 (rd-1028/brief-w1-2-fix1.md): the same restart with `stop_emulator`: the
//!   wake's configured start finds the emulator the first wake started running, and still queues
//!   the startup package, which runs once after the restart and the pause's end. Adapted for it
//!   (rd-1028 W1-2 fix round 2): N-1 with `stop_emulator`, whose wake's start finds X's emulator
//!   running, now sees that start queue the startup package once, and X stays woken until the
//!   startup claim has given X's key up (O-2), so the next plan is checked only after that.
//! - N-2: the record is bound to the start anchor (`host.contained_settlement`), not to the latest
//!   owner: an owner in between that settled no start leaves it current. Adapted for it: the
//!   writer row `kept_record` and `kept_history` read (`start_anchor`, was `owner_epoch`), and M-2's
//!   "its own epoch" is now its own anchor.
//! - N-5: both failures of an action carried out are reported, through the helper N-3's use.
//!
//! L-1 to L-3 go through the host with the scripted emulator of `gate_idle1_host` (X's idle action
//! `stop_emulator`, its configured emulator start and a startup package) on a manual clock; each
//! restart starts a new host on the same state root. L-5 and L-6 read the plan the idle check
//! reads from a recorded evaluation (`ScheduleExplanationTable`) on a manual clock. Every wait
//! returns as soon as what it waits for has happened; `WAIT` only bounds a broken run.

use super::emulator_controller::{EmulatorCall, FAKE_EMULATOR_PORT, running_state, stopped_state};
use super::gate_369_s6c::{connectivity, head, instance_scope, of_request, resume, send};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation};
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, KeptIdle, WakeWork};
use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::{
    EmulatorInstanceAction, FactScalar, FactValue, InstanceStuckRecovery,
    MAX_RUNTIME_SUBSCRIPTION_WAIT_MS, RequestId, RuntimeConfigManifest, RuntimeFactScope,
    RuntimeLifecyclePhase, RuntimePayload, RuntimeSubscriptionRequest, SchedulingPauseReason,
    SubscriptionCursor,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_policy::{
    EligibilityState, FACT_REFRESH_TASK_ID, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision, TaskRank,
};
use actingcommand_runtime_config::{
    DefaultsFile, IdleAction, IdleConfig, annotate_settable, mechanism_rows, render_parameters,
};
use actingcommand_scheduler::ClaimKind;
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
    action_manifest(x, "stop_emulator")
}

/// [`idle_manifest`] with the idle action `action`.
fn action_manifest(x: InstanceId, action: &str) -> RuntimeConfigManifest {
    let id = instance_text(x);
    let file = json!({"instances": [{
        "instance_id": id,
        "start_emulator": true,
        "idle": {"action": action}
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

/// `host.idle_state` as the host holds it: the start anchor its writer row names (#405 N-2;
/// review M-2 named the owner epoch), and its instance rows.
fn kept_record(host: &RuntimeHost) -> Option<(i64, Vec<BTreeMap<String, FactScalar>>)> {
    host.runtime_fact_snapshot()
        .expect("the program facts")
        .records
        .into_iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == IDLE_STATE)
        .map(|record| match record.value {
            FactValue::RecordList(mut rows) => {
                assert!(!rows.is_empty(), "host.idle_state has its writer row");
                let writer = rows.remove(0);
                let anchor = match (writer.len(), writer.get("start_anchor")) {
                    (1, Some(FactScalar::Integer(anchor))) => *anchor,
                    _ => panic!("the first row names the writer's start_anchor alone: {writer:?}"),
                };
                (anchor, rows)
            }
            other => panic!("host.idle_state is a record list: {other:?}"),
        })
}

/// X's row: closed by `stop_emulator`, its wake `wake` when one is planned.
fn x_row(wake: Option<u64>) -> BTreeMap<String, FactScalar> {
    x_row_of("stop_emulator", wake)
}

/// X's row: closed by `action`, its wake `wake` when one is planned.
fn x_row_of(action: &str, wake: Option<u64>) -> BTreeMap<String, FactScalar> {
    let mut row = BTreeMap::from([
        (
            "instance_alias".to_owned(),
            FactScalar::String(X_ALIAS.to_owned()),
        ),
        ("action".to_owned(), FactScalar::String(action.to_owned())),
    ]);
    if let Some(at) = wake {
        row.insert("wake_at_unix_ms".to_owned(), FactScalar::TimestampMs(at));
    }
    row
}

/// #405 O-2: X's row while it is woken and its startup package has not run: closed by
/// `stop_emulator`, woken at `at` (its wake).
fn x_woken_row(at: u64) -> BTreeMap<String, FactScalar> {
    x_woken_row_of("stop_emulator", at)
}

/// #405 O-2: X's row while it is woken and its startup package has not run: closed by `action`,
/// woken at `at` (its wake).
fn x_woken_row_of(action: &str, at: u64) -> BTreeMap<String, FactScalar> {
    let mut row = x_row_of(action, Some(at));
    row.insert("woken".to_owned(), FactScalar::Boolean(true));
    row
}

/// Review N-1: the instance rows of every `host.idle_state` the host's own start recorded (#405
/// N-2: under its start anchor), in order (the start's restore first). Read from the ledger, so a
/// check of the lease sweep that took a wake before the test's own check changes nothing here.
fn kept_history(host: &RuntimeHost) -> Vec<Vec<BTreeMap<String, FactScalar>>> {
    let (anchor, _) = kept_record(host).expect("the host keeps its state");
    let writer = FactScalar::Integer(anchor);
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == IDLE_STATE =>
            {
                match &recorded.record().value {
                    FactValue::RecordList(rows) => {
                        let (first, rows) = rows.split_first()?;
                        (first.get("start_anchor") == Some(&writer)).then(|| rows.to_vec())
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// #405 O-2: waits until the configured emulator start X's wake queued has ended (one of its
/// report lines); a start that found nothing to start, or failed, released X's key before it.
fn await_wake_start_end(host: &RuntimeHost) {
    host.emulator_autostart_reports()
        .await_lines_for_test(
            |lines| {
                lines
                    .iter()
                    .any(|line| AUTOSTART_ENDS.iter().any(|code| line.is(*code)))
            },
            WAIT,
        )
        .expect("the wake's configured emulator start ended");
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
    // #405 O-2 (0.12.25, adapted): woken, X stays kept closed and woken while its wake's claims
    // hold it (what follows depends on how they end; review L-1: the history's first two records).
    assert_eq!(
        kept_history(&host)[..2],
        [
            vec![x_row(Some(WAKE_UNIX_MS))],
            vec![x_woken_row(WAKE_UNIX_MS)]
        ],
        "X is restored closed with its wake, then woken"
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

    let started_at = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(started_at);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the start");
    // The start's first check, before any evaluated cycle (the lease sweep's may be the one that
    // takes it): X is woken, never acted on.
    host.check_idle_instances_for_test(&no_plan())
        .expect("the start's check");
    // #405 O-2 (0.12.25, adapted): woken, X stays kept closed and woken while its wake's claims
    // hold it (review L-1: the history's first two records).
    assert_eq!(
        kept_history(&host)[..2],
        [vec![x_row(Some(started_at))], vec![x_woken_row(started_at)]],
        "X, whose state the start does not know, counts as closed with its wake at the start \
         (review N-1), then is woken"
    );
    assert_eq!(stops(&state), 0, "X is not treated as open");
    assert_eq!(
        idle_records(&host, x, 0),
        wake_records(),
        "one wake at the start"
    );
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
    // v0.12.20: the ledger's first start keeps X open (nothing closed) under its own anchor.
    let first = start(&root, &clock, x, &x_state(), false).expect("the first start");
    let (first_anchor, rows) = kept_record(&first).expect("the first start keeps its state");
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
    // The start's first check, before any evaluated cycle (the lease sweep's may take it).
    host.check_idle_instances_for_test(&no_plan())
        .expect("the start's check");
    let (anchor, _) = kept_record(&host).expect("the start keeps its state");
    assert_ne!(
        anchor, first_anchor,
        "the start keeps its state under its own anchor (#405 N-2; it was the owner epoch)"
    );
    // #405 O-2 (0.12.25, adapted): woken, X stays kept closed and woken while its wake's claims
    // hold it (review L-1: the history's first two records).
    assert_eq!(
        kept_history(&host)[..2],
        [vec![x_row(Some(now))], vec![x_woken_row(now)]],
        "X, whose record is stale, counts as closed with its wake at the start (review N-1), \
         then is woken"
    );
    assert_eq!(stops(&state), 0, "X is not treated as open");
    let mut expected = vec![action_record()];
    expected.extend(wake_records());
    assert_eq!(
        idle_records(&host, x, 0),
        expected,
        "the older build's action, then one wake at the start"
    );
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

// ---- Review N-1 (round 2): a start without a usable record wakes each closed instance once ----

/// How the configured emulator start a wake queued can end (one report line each).
const AUTOSTART_ENDS: [HostCode; 7] = [
    HostCode::EmulatorAutostartSkipped,
    HostCode::EmulatorAutostartFailed,
    HostCode::EmulatorAutostartStarted,
    HostCode::EmulatorAutostartPreparationFailed,
    HostCode::EmulatorAutostartSuperseded,
    HostCode::EmulatorAutostartInterrupted,
    HostCode::EmulatorAutostartNetworkUnready,
];

/// X's work while its status reading has expired at a start: the reading held back by the idle
/// action past its own gates (eligible, deferred, no wake), and X's only task, whose trigger reads
/// the status, waiting for it undecided with no wake (`fact_refresh_pending`, `gate_d2_s4`).
fn status_expired() -> Vec<TaskDecision> {
    vec![
        decision(
            FACT_REFRESH_TASK_ID,
            EligibilityState::True,
            SchedulingDecisionState::Deferred,
            None,
        ),
        decision(
            "task.reads_the_status",
            EligibilityState::Unknown,
            SchedulingDecisionState::Deferred,
            None,
        ),
    ]
}

/// X's work once the reading is fresh until `expiry`: the reading not due before then, and the
/// task false, waking at `expiry + 1` (a fresh `fact` predicate's wake).
fn status_fresh(expiry: u64) -> Vec<TaskDecision> {
    vec![
        decision(
            FACT_REFRESH_TASK_ID,
            EligibilityState::False,
            SchedulingDecisionState::Blocked,
            Some(expiry),
        ),
        decision(
            "task.reads_the_status",
            EligibilityState::False,
            SchedulingDecisionState::Blocked,
            Some(expiry + 1),
        ),
    ]
}

/// X's plan from an evaluation of `decisions` (made X's) at `at`, as the idle check reads it.
fn x_explained(decisions: Vec<TaskDecision>, at: u64) -> BTreeMap<String, IdlePlan> {
    let decisions = decisions
        .into_iter()
        .map(|decision| TaskDecision {
            instance_id: Some(X_ALIAS.to_owned()),
            ..decision
        })
        .collect();
    let plan = explained_on(X_ALIAS, decisions, at).expect("an evaluated plan");
    BTreeMap::from([(X_ALIAS.to_owned(), plan)])
}

/// The one wake record of X after `close_app` (its wake queues the startup package alone).
fn close_app_wake_record() -> String {
    format!(
        "{} {}",
        IdleCode::Taken.as_str(),
        IdleLocation::Wake.as_str()
    )
}

/// The request of X's startup claim: the claim requested under the causation of X's latest
/// `startup_package_scheduled`.
fn startup_claim(host: &RuntimeHost, x: InstanceId) -> RequestId {
    let events = all_events(host);
    let causation = phases(&events)
        .into_iter()
        .rev()
        .find(|(_, phase)| {
            matches!(
                phase,
                RuntimeLifecyclePhase::StartupPackageScheduled { instance_id } if *instance_id == x
            )
        })
        .and_then(|(event, _)| event.links().causation_id().copied())
        .expect("X's startup package is scheduled");
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

/// Whether a record of `event_type` says the claim held the key: granted, handed it by a
/// transfer, or released after its run.
const fn held_the_key(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::LeaseGranted | EventType::LeaseTransferred | EventType::LeaseReleased
    )
}

/// How many of the claim's records say it held the key.
fn key_records(host: &RuntimeHost, claim: RequestId) -> usize {
    of_request(host, claim)
        .iter()
        .filter(|event| held_the_key(event.event_type()))
        .count()
}

/// Waits until the claim's records say it held the key; each wait is a ledger subscription that
/// answers as soon as the claim's next record is written. One subscription waits at most the
/// contract's longest wait, so `WAIT`, which only bounds a broken run, allows that many
/// consecutive empty answers.
fn await_key(host: &RuntimeHost, claim: RequestId) {
    let empty_answers = WAIT
        .as_millis()
        .div_ceil(u128::from(MAX_RUNTIME_SUBSCRIPTION_WAIT_MS));
    let mut empty = 0;
    let mut after = 0;
    loop {
        let query = EventQuery {
            request_id: Some(claim),
            from_sequence: Some(after + 1),
            ..EventQuery::default()
        };
        let subscription = RuntimeSubscriptionRequest::new(
            query.clone(),
            ProjectionProfile::Forensic,
            SubscriptionCursor::default(),
            MAX_RUNTIME_SUBSCRIPTION_WAIT_MS,
            1,
        )
        .expect("subscription request");
        let receipt = send(
            host,
            RuntimeOperation::SubscribeEvents {
                request: subscription,
            },
            4_403,
        );
        let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
            panic!("an event batch: {receipt:?}");
        };
        if batch.timed_out() {
            empty += 1;
            assert!(empty < empty_answers, "the startup claim's next record");
            continue;
        }
        empty = 0;
        let next = host
            .query_persisted_events_for_test(query)
            .expect("the claim's records")
            .into_iter()
            .next()
            .expect("the claim's next record");
        if held_the_key(next.event_type()) {
            return;
        }
        after = next.sequence();
    }
}

/// #405 O-2, review M-1: waits until X's startup claim `claim` has given X's key up and X is free.
/// After the claim was requested, X's key is released twice: by the wake's configured emulator
/// start, which hands it to the claim, then by the claim (its run's release, completed or failed,
/// or the claim's own after a refused preparation). A release is recorded once the key is free;
/// the release holds X's admission guard until it returns, so the guard is then taken and let go.
fn await_startup_claim_end(host: &RuntimeHost, x: InstanceId, claim: RequestId) {
    let mut after = of_request(host, claim)
        .first()
        .map(PersistedEvent::sequence)
        .expect("the startup claim's request");
    for release in ["the wake's start's release", "the startup claim's release"] {
        after = await_release(host, x, after, release);
    }
    let admission = host
        .instance_admission_for_test(X_ALIAS)
        .expect("X's admission guard");
    drop(admission.lock().expect("X's admission guard"));
}

/// The sequence of X's next key release after `after` (`what`); each wait is a ledger subscription
/// that answers as soon as one is written, as in [`await_key`].
fn await_release(host: &RuntimeHost, x: InstanceId, after: u64, what: &str) -> u64 {
    let query = EventQuery {
        event_type: Some(EventType::LeaseReleased),
        instance_id: Some(x),
        from_sequence: Some(after + 1),
        ..EventQuery::default()
    };
    let empty_answers = WAIT
        .as_millis()
        .div_ceil(u128::from(MAX_RUNTIME_SUBSCRIPTION_WAIT_MS));
    for _ in 0..empty_answers {
        let subscription = RuntimeSubscriptionRequest::new(
            query.clone(),
            ProjectionProfile::Forensic,
            SubscriptionCursor::default(),
            MAX_RUNTIME_SUBSCRIPTION_WAIT_MS,
            1,
        )
        .expect("subscription request");
        let receipt = send(
            host,
            RuntimeOperation::SubscribeEvents {
                request: subscription,
            },
            4_404,
        );
        let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
            panic!("an event batch: {receipt:?}");
        };
        if !batch.timed_out() {
            return host
                .query_persisted_events_for_test(query)
                .expect("X's key releases")
                .first()
                .map(PersistedEvent::sequence)
                .expect(what);
        }
    }
    panic!("{what}: X's key was not released within the wait");
}

/// X paused with `reason`, as a person or the installer's hold pauses it.
fn pause_x(host: &RuntimeHost, reason: &str) {
    let receipt = send(
        host,
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(X_ALIAS),
            pause_reason: SchedulingPauseReason::new(reason).expect("pause reason"),
            drain_timeout_ms: 5_000,
        },
        4_401,
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// Review N-1 through the host on X (`stop_emulator`): a start that cannot use a record (an
/// earlier start kept none), with X's only work waiting for a status reading that expired
/// meanwhile. Its plan has no task due and no task wake, and the reading alone wakes nothing, yet
/// X is woken once at the start's first check. Its wake's configured emulator start finds X's
/// emulator running and starts nothing; since review M-1 (0.12.25) it still queues X's startup
/// package once, and X stays woken until that claim has given X's key up (#405 O-2). The next
/// plan, after the reading, then closes X again with its proper wake: the task's next wake minus
/// the lead.
#[test]
fn idle1p_n1_an_unknown_start_wakes_an_instance_whose_work_waits_for_an_expired_reading() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    // An earlier start that kept no idle state (it fails right after its start event).
    let Err(_) = start(&root, &clock, x, &x_state(), true) else {
        panic!("the earlier start did not fail after its start event");
    };

    let started_at = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(started_at);
    let state = x_state();
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    let host = start(&root, &clock, x, &state, false).expect("the start");
    let expired = x_explained(status_expired(), started_at);
    let plan = expired[X_ALIAS];
    assert!(
        !plan.due_now && plan.next_wake_unix_ms.is_none() && plan.reading_due_unix_ms.is_some(),
        "no task due or waking, the reading held back: {plan:?}"
    );
    // The start's first check (the lease sweep's may be the one that takes the wake).
    host.check_idle_instances_for_test(&expired)
        .expect("the start's check");
    // #405 O-2 (0.12.25, adapted): woken, X is kept closed and woken until its wake's claims
    // have ended (what follows depends on which check finds it free first).
    assert_eq!(
        kept_history(&host)[..2],
        [vec![x_row(Some(started_at))], vec![x_woken_row(started_at)]],
        "X is restored closed with its wake at the start, then woken"
    );
    assert_eq!(
        idle_records(&host, x, 0),
        wake_records(),
        "one wake at the start's first check"
    );
    await_wake_start_end(&host);
    assert!(
        !state
            .emulator
            .calls()
            .contains(&EmulatorCall::Control(EmulatorInstanceAction::Start)),
        "X's running emulator is not started"
    );
    // Review M-1 (0.12.25, adapted): the wake's start, which found X's emulator running, still
    // queued the startup package once, and X stays woken while that claim holds it (#405 O-2):
    // the next plan meets X free only once the claim has given X's key up.
    assert_eq!(
        startups_scheduled(&host, x, 0),
        1,
        "the wake's start queues the startup package once"
    );
    await_startup_claim_end(&host, x, startup_claim(&host, x));

    // The next plan, after the reading: the task wakes at the fresh status's expiry + 1.
    let fresh_at = started_at + MINUTE_MS;
    clock.set_unix_ms(fresh_at);
    let expiry = fresh_at + 4 * 60 * MINUTE_MS;
    host.check_idle_instances_for_test(&x_explained(status_fresh(expiry), fresh_at))
        .expect("the next check");
    assert_eq!(
        host.await_idle_action_ends_for_test(1, WAIT),
        Some(1),
        "X's idle action ended"
    );
    assert_eq!(stops(&state), 1, "X is closed again");
    assert_eq!(
        kept_rows(&host),
        Some(vec![x_row(Some(expiry + 1 - STOP_EMULATOR_LEAD_MS))]),
        "X is kept closed with its proper wake"
    );
    let mut expected = wake_records();
    expected.push(action_record());
    assert_eq!(idle_records(&host, x, 0), expected);
    host.close().expect("close the host");
}

/// Review N-1 through the host on X with the production action `close_app` (its wake is its
/// startup package alone), at the start that follows an older build (the v0.12.19 → v0.12.20
/// deploy: no record), with X's only work waiting for an expired status reading. X is woken at the
/// start's first check and its startup package is claimed. Under a pause of X restored at the
/// start (a person's standing pause, or the install hold, which pauses every instance with its
/// reason) the wake neither overrides it nor is lost: the startup claim waits queued, a later
/// check claims nothing more, nothing is granted until the pause ends, and the resume grants it.
#[test]
fn idle1p_n1_the_wakes_startup_claim_waits_behind_a_pause_or_the_install_hold() {
    for held in [
        None,
        Some("idle1p_standing_pause"),
        Some("idle1p_install_hold"),
    ] {
        let root = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
        let x = instance_id();
        let close_app = |config: RuntimeHostConfig| {
            config.with_config_manifest(action_manifest(x, "close_app"))
        };
        // The older build keeps no idle state; the pause is set before its stop.
        let older = start_with(&root, &clock, x, &x_state(), |config| {
            close_app(config).with_idle_state_unkept_for_test()
        })
        .expect("the older build's start");
        if let Some(reason) = held {
            pause_x(&older, reason);
        }
        older.close().expect("close the older build");

        let started_at = NOW_UNIX_MS + MINUTE_MS;
        clock.set_unix_ms(started_at);
        let host = start_with(&root, &clock, x, &x_state(), close_app).expect("the start");
        let (_, pauses) = host.scheduling_pauses_for_test().expect("pauses");
        assert_eq!(
            pauses.contains_key(X_ALIAS),
            held.is_some(),
            "{held:?}: X's pause is restored"
        );
        let expired = x_explained(status_expired(), started_at);
        host.check_idle_instances_for_test(&expired)
            .expect("the start's check");
        // #405 O-2 (0.12.25, adapted): woken, X is kept closed and woken until its startup
        // package has run.
        assert_eq!(
            kept_history(&host)[..2],
            [
                vec![x_row_of("close_app", Some(started_at))],
                vec![x_woken_row_of("close_app", started_at)]
            ],
            "{held:?}: X is restored closed with its wake at the start, then woken"
        );
        assert_eq!(
            idle_records(&host, x, 0),
            [close_app_wake_record()],
            "{held:?}: one wake"
        );
        let claim = startup_claim(&host, x);
        if held.is_some() {
            let waiting = (None, vec![ClaimKind::StartupPackage]);
            assert_eq!(
                host.instance_claims_for_test(X_ALIAS).expect("claims"),
                waiting,
                "{held:?}: the startup claim waits behind the pause"
            );
            host.check_idle_instances_for_test(&expired)
                .expect("a later check");
            assert_eq!(
                host.instance_claims_for_test(X_ALIAS).expect("claims"),
                waiting,
                "{held:?}: a later check claims nothing more"
            );
            assert_eq!(
                idle_records(&host, x, 0),
                [close_app_wake_record()],
                "{held:?}: and wakes nothing again"
            );
            assert_eq!(
                key_records(&host, claim),
                0,
                "{held:?}: the startup claim never held the key"
            );
            let resumed = resume(&host, instance_scope(X_ALIAS), 4_402);
            assert_eq!(
                resumed.state(),
                RuntimeReceiptState::Completed,
                "{held:?}: {resumed:?}"
            );
        }
        // The startup claim gets the key: at once, or once the pause ends.
        await_key(&host, claim);
        host.close().expect("close the host");
    }
}

/// Review N-3: an idle action abandoned on its key (it met a pause or work) whose key release and
/// `host.idle_state` record both fail: each failure is recorded under X through the one decision
/// point, the release's first, and neither stops the Runtime.
#[test]
fn idle1p_n3_both_failures_of_an_abandoned_action_are_reported() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let host = start(&root, &clock, x, &x_state(), false).expect("the start");
    let before = head(&host);
    let (release, record) = (HostCode::ReleaseTimeout, HostCode::RuntimeFactStale);
    host.contain_abandoned_idle_ends_for_test(X_ALIAS, Some(release), Some(record))
        .expect("both are failures of X's action alone");
    let failures = all_events(&host)
        .iter()
        .filter(|event| event.sequence() > before && event.links().instance_id() == Some(&x))
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(failed)) => {
                Some(failed.outcome().code().as_str().to_owned())
            }
            _ => None,
        })
        .filter(|code| *code == release.as_str() || *code == record.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        failures,
        [release.as_str(), record.as_str()],
        "the release's failure, then the record's"
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "neither latches the Runtime"
    );
    host.close().expect("close the host");
}

// ---- 0.12.25 (#405 O-2, N-2, N-5; brief rd-1028/brief-wave1.md W1-2) ----

/// #405 O-2 with the production action `close_app`: X's wake falls due while a pause holds X (a
/// person's standing pause; a deploy's hold pauses every instance alike), so its startup package is
/// only queued in memory. X stays kept closed and woken while the claim waits. A restart then
/// loses the queue but not the wake: the restart restores X closed, its first check wakes X again,
/// and once the pause ends the startup package runs after the restart.
#[test]
fn idle1p_o2_a_restart_while_the_wake_is_queued_runs_the_startup_package_after_it() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let close_app =
        |config: RuntimeHostConfig| config.with_config_manifest(action_manifest(x, "close_app"));
    // An older build keeps no idle state, so the next start does not know X's (review N-1) and X
    // owes its wake at that start; the pause is set before the older build's stop.
    let older = start_with(&root, &clock, x, &x_state(), |config| {
        close_app(config).with_idle_state_unkept_for_test()
    })
    .expect("the older build's start");
    pause_x(&older, "idle1p_o2_standing_pause");
    older.close().expect("close the older build");

    // The start: X's wake falls due at once, while the pause holds X.
    let woken_at = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(woken_at);
    let first = start_with(&root, &clock, x, &x_state(), close_app).expect("the start");
    first
        .check_idle_instances_for_test(&no_plan())
        .expect("the start's check");
    assert_eq!(
        idle_records(&first, x, 0),
        [close_app_wake_record()],
        "one wake at the start"
    );
    assert_eq!(
        first.instance_claims_for_test(X_ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the pause, in memory only"
    );
    assert_eq!(
        kept_rows(&first),
        Some(vec![x_woken_row_of("close_app", woken_at)]),
        "X is kept closed and woken while its startup package has not run"
    );

    // The restart while the claim waits.
    first.close().expect("close the start");
    clock.set_unix_ms(woken_at + MINUTE_MS);
    let host = start_with(&root, &clock, x, &x_state(), close_app).expect("the restart");
    let (_, pauses) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(pauses.contains_key(X_ALIAS), "X's pause is restored again");
    assert_eq!(
        kept_history(&host)[0],
        vec![x_row_of("close_app", Some(woken_at))],
        "the restart restores X closed, owing its wake at the time it was woken"
    );
    // The restart's first check (the lease sweep's may be the one that takes the wake).
    host.check_idle_instances_for_test(&no_plan())
        .expect("the restart's check");
    assert_eq!(
        idle_records(&host, x, 0),
        [close_app_wake_record(), close_app_wake_record()],
        "one wake at the start, and X is woken again at the restart"
    );
    let claim = startup_claim(&host, x);
    assert_eq!(
        key_records(&host, claim),
        0,
        "the restored pause holds the startup claim"
    );
    let resumed = resume(&host, instance_scope(X_ALIAS), 4_402);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    // The startup package runs after the restart: its claim gets the key once the pause ends.
    await_key(&host, claim);
    host.close().expect("close the restarted host");
}

/// The Starts X's scripted emulator was asked for.
fn emulator_starts(state: &FakeState) -> usize {
    state
        .emulator
        .calls()
        .into_iter()
        .filter(|call| *call == EmulatorCall::Control(EmulatorInstanceAction::Start))
        .count()
}

/// How many times X's startup package was scheduled after `after` (`startup_package_scheduled`).
fn startups_scheduled(host: &RuntimeHost, x: InstanceId, after: u64) -> usize {
    phases(&all_events(host))
        .into_iter()
        .filter(|(event, phase)| {
            event.sequence() > after
                && matches!(
                    phase,
                    RuntimeLifecyclePhase::StartupPackageScheduled { instance_id }
                        if *instance_id == x
                )
        })
        .count()
}

/// #405 O-2, review M-1 (rd-1028/brief-w1-2-fix1.md): the same restart with `stop_emulator`. X's
/// wake falls due while a pause holds X (a person's standing pause; a deploy's hold pauses every
/// instance alike). Its configured emulator start, which a pause never holds (ruling 35), starts
/// X's emulator, and the startup package's claim waits behind the pause in memory. A restart then
/// loses that claim but not the wake: the restart restores X closed, its first check wakes X
/// again, and the wake's start finds the emulator running. It starts nothing and still queues the
/// startup package, once, which gets the key once the pause ends.
#[test]
fn idle1p_m1_a_stop_emulator_restart_while_the_wake_is_queued_runs_the_startup_package_after_it() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    // An older build keeps no idle state, so the next start does not know X's (review N-1) and X
    // owes its wake at that start; the pause is set before the older build's stop.
    let older = start_with(
        &root,
        &clock,
        x,
        &x_state(),
        RuntimeHostConfig::with_idle_state_unkept_for_test,
    )
    .expect("the older build's start");
    pause_x(&older, "idle1p_m1_standing_pause");
    older.close().expect("close the older build");

    // The start: X's wake falls due at once, while the pause holds X. Its configured emulator
    // start reads X's emulator stopped and starts it; the startup claim waits behind the pause.
    let woken_at = NOW_UNIX_MS + MINUTE_MS;
    clock.set_unix_ms(woken_at);
    let state = x_state();
    state.emulator.script_read(stopped_state());
    let first = start(&root, &clock, x, &state, false).expect("the start");
    first
        .check_idle_instances_for_test(&no_plan())
        .expect("the start's check");
    assert_eq!(
        idle_records(&first, x, 0),
        wake_records(),
        "one wake at the start"
    );
    await_wake_start_end(&first);
    assert_eq!(
        emulator_starts(&state),
        1,
        "the wake's start starts X's emulator under the pause"
    );
    assert_eq!(
        first.instance_claims_for_test(X_ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the pause, in memory only"
    );
    assert_eq!(
        kept_rows(&first),
        Some(vec![x_woken_row(woken_at)]),
        "X is kept closed and woken while its startup package has not run"
    );

    // The restart while the claim waits, X's emulator running as the first wake left it.
    first.close().expect("close the start");
    clock.set_unix_ms(woken_at + MINUTE_MS);
    let state = x_state();
    state
        .emulator
        .script_read(running_state(FAKE_EMULATOR_PORT));
    let host = start(&root, &clock, x, &state, false).expect("the restart");
    let restarted = head(&host);
    let (_, pauses) = host.scheduling_pauses_for_test().expect("pauses");
    assert!(pauses.contains_key(X_ALIAS), "X's pause is restored again");
    assert_eq!(
        kept_history(&host)[0],
        vec![x_row(Some(woken_at))],
        "the restart restores X closed, owing its wake at the time it was woken"
    );
    // The restart's first check (the lease sweep's may be the one that takes the wake).
    host.check_idle_instances_for_test(&no_plan())
        .expect("the restart's check");
    let mut woken_twice = wake_records();
    woken_twice.extend(wake_records());
    assert_eq!(
        idle_records(&host, x, 0),
        woken_twice,
        "one wake at the start, and X is woken again at the restart"
    );
    // The wake's start finds the emulator running: it starts nothing and still queues the
    // startup package (review M-1), which the restored pause holds.
    await_wake_start_end(&host);
    assert_eq!(
        emulator_starts(&state),
        0,
        "X's running emulator is not started again"
    );
    assert_eq!(
        host.instance_claims_for_test(X_ALIAS).expect("claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "the startup claim waits behind the restored pause"
    );
    assert_eq!(
        startups_scheduled(&host, x, restarted),
        1,
        "the restart's wake queues the startup package once"
    );
    let claim = startup_claim(&host, x);
    assert_eq!(
        key_records(&host, claim),
        0,
        "the restored pause holds the startup claim"
    );
    let resumed = resume(&host, instance_scope(X_ALIAS), 4_402);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    // The startup package runs after the restart: its claim gets the key once the pause ends.
    await_key(&host, claim);
    assert_eq!(
        startups_scheduled(&host, x, restarted),
        1,
        "the startup package is queued once after the restart"
    );
    host.close().expect("close the restarted host");
}

/// #405 N-2: the record is bound to the start anchor (`host.contained_settlement`), not to the
/// latest owner. An owner that ran no Runtime start to its settlement in between (an offline
/// ledger maintenance or a deploy's ledger check takes the owner so; here a start that fails right
/// after its start event, before its settlement) leaves the record current: the restart restores
/// X closed with its kept wake, and X is neither woken at the restart nor acted on again.
#[test]
fn idle1p_n2_an_owner_that_settled_no_start_in_between_leaves_the_record_current() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let (first, _) = close_x(&root, &clock, x);
    let (first_anchor, _) = kept_record(&first).expect("the first start keeps its state");
    first.close().expect("close the first host");

    // The owner in between: it appends its start event and fails before its settlement.
    clock.set_unix_ms(NOW_UNIX_MS + 10 * MINUTE_MS);
    let Err(_) = start(&root, &clock, x, &x_state(), true) else {
        panic!("the start in between did not fail after its start event");
    };

    // The restart, well before the wake.
    clock.set_unix_ms(NOW_UNIX_MS + 30 * MINUTE_MS);
    let state = x_state();
    let host = start(&root, &clock, x, &state, false).expect("the restart");
    host.check_idle_instances_for_test(&no_plan())
        .expect("the restart's check");
    assert_eq!(
        kept_history(&host),
        vec![vec![x_row(Some(WAKE_UNIX_MS))]],
        "the record is current: X is restored closed with its kept wake"
    );
    let (anchor, _) = kept_record(&host).expect("the restart keeps its state");
    assert_ne!(
        anchor, first_anchor,
        "the restart keeps it again under its own anchor"
    );
    assert_eq!(
        idle_records(&host, x, 0),
        [action_record()],
        "the first start's action alone: no wake at the restart"
    );
    assert_eq!(stops(&state), 0, "X is not acted on again");
    host.close().expect("close the restarted host");
}

/// #405 N-5: an idle action carried out (its `finished` path) whose key release and
/// `host.idle_state` record both fail: both go through the helper an abandoned action's take, so
/// each failure is recorded under X and the action's links, the release's first, the action ends
/// before its own record, and neither stops the Runtime.
#[test]
fn idle1p_n5_both_failures_of_a_finished_action_are_reported() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(NOW_UNIX_MS, 0));
    let x = instance_id();
    let host = start(&root, &clock, x, &x_state(), false).expect("the start");
    let (release, record) = (HostCode::ReleaseTimeout, HostCode::RuntimeFactStale);
    let (causation, stopped) = host
        .contain_finished_idle_ends_for_test(X_ALIAS, Some(release), Some(record))
        .expect("both are failures of X's action alone");
    assert!(stopped, "the action ends before its own record");
    let failures = all_events(&host)
        .iter()
        .filter(|event| {
            event.links().instance_id() == Some(&x)
                && event.links().causation_id() == Some(&causation)
        })
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(failed)) => {
                Some(failed.outcome().code().as_str().to_owned())
            }
            _ => None,
        })
        .filter(|code| *code == release.as_str() || *code == record.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        failures,
        [release.as_str(), record.as_str()],
        "the release's failure, then the record's, under the action's links"
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "neither latches the Runtime"
    );
    let (_, stopped) = host
        .contain_finished_idle_ends_for_test(X_ALIAS, None, None)
        .expect("no failure");
    assert!(
        !stopped,
        "with both ends done the action goes on to its record"
    );
    host.close().expect("close the host");
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
    explained_on(NODE, decisions, at)
}

/// The idle check's plan for `alias` from an evaluation of `decisions` at `at`.
fn explained_on(alias: &str, decisions: Vec<TaskDecision>, at: u64) -> Option<IdlePlan> {
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
    table.idle_plan(alias)
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

/// #405 O-2 (0.12.25), the planner: an instance woken while a pause holds it stays kept closed and
/// woken (`woken`, its wake the time it was woken) while its wake's claims hold it (its startup
/// claim queued behind the pause, then its run on the key), and is no longer kept closed once it is
/// free; restored from that record, it is woken again at once.
#[test]
fn idle1p_o2_a_woken_instance_is_kept_closed_until_its_wakes_claims_end() {
    let next_due = T0 + 4 * 60 * MINUTE_MS;
    let wake = next_due - LEAD;
    let plan = |at: u64| explained(vec![later_task(next_due)], at);
    let paused = IdleView {
        paused: true,
        ..free()
    };
    let mut planner = closed(next_due);
    // The wake falls due while a pause holds the instance: it is queued all the same (IDLE-2).
    assert_eq!(
        planner.check(NODE, &PRODUCTION, paused, plan(wake), wake),
        Some(WAKE)
    );
    let woken = KeptIdle {
        action: IdleAction::CloseApp,
        wake_at_unix_ms: Some(wake),
        woken: true,
    };
    let kept = BTreeMap::from([(NODE.to_owned(), woken)]);
    assert_eq!(
        planner.closed(),
        kept,
        "closed and woken: its startup package is only queued"
    );
    // Its startup claim waits behind the pause, then runs on the key: still closed and woken.
    for (at, view) in [
        (
            wake + MINUTE_MS,
            IdleView {
                queued: true,
                ..paused
            },
        ),
        (
            wake + 2 * MINUTE_MS,
            IdleView {
                leased: true,
                ..free()
            },
        ),
    ] {
        assert_eq!(
            planner.check(NODE, &PRODUCTION, view, plan(at), at),
            None,
            "{view:?}"
        );
        assert_eq!(planner.closed(), kept, "{view:?}: kept closed and woken");
    }
    // A restart meanwhile restores it owing its wake at the time it was woken: the start's first
    // check, before any plan, wakes it again, so its startup package is queued again.
    let mut restarted = IdlePlanner::default();
    restarted.restore(NODE, woken);
    let start = wake + 3 * MINUTE_MS;
    assert_eq!(
        restarted.check(NODE, &PRODUCTION, paused, None, start),
        Some(WAKE)
    );
    // Free once its startup package has run: open again, no longer kept closed, and not closed
    // again with its next task inside the minimum gap.
    let ran = wake + 4 * MINUTE_MS;
    assert_eq!(
        planner.check(NODE, &PRODUCTION, free(), plan(ran), ran),
        None
    );
    assert!(planner.closed().is_empty(), "no longer kept closed");
}
