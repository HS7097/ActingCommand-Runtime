// SPDX-License-Identifier: AGPL-3.0-only

//! #486 ASLEEP-1 (0.12.77; register §1.497; `contracts/runtime-config.md`, "What wakes an idle
//! instance"; `contracts/scheduling/README.md`, "An asleep instance"), the host half. An instance
//! whose emulator its idle action stopped is asleep, not unavailable: the evaluation judges it as
//! if it were available and selects nothing on it, and the idle plan follows; a wake whose work
//! started the emulator must be answered by a scheduled run admitted on the instance before the
//! emulator is stopped again (ruling 6).
//!
//! The fixture instance is `gate_372k2`'s policy instance on a host of its own, emulator-controlled
//! and device self-checked (its availability is its self-check's, as `gate_484_idle_w2`'s
//! `close_app` fixture's), with no idle action of its own configured: the host's own idle check
//! leaves it alone, and the test runs the pure idle planner under `stop_emulator` on the plans the
//! host's evaluations leave (`RuntimeHost::idle_plan_and_runs_counted_for_test`), as
//! `gate_484_idle_w2` does. The stop's two ends on the host are what the work order's ruling 1
//! reads: the planner's idle phase after `stop_emulator` (`hold_emulator_stopped_for_test`) and the
//! availability the stop's invalidation of the self-check withdraws through the backend self-check
//! gate (`backend_selfcheck:unchecked`, `publish_backend_selfcheck_unavailable_for_test`). Every
//! evaluation runs on a manual Runtime clock; nothing reads a clock.
//!
//! - 5: `stop_emulator` stops the emulator with its wake at the spent budget's release less the
//!   lead; asleep, the evaluation defers the task by its budget, not `instance_unavailable`, and
//!   no check starts the emulator again.
//! - 6: the same with a candidate whose benefit is unknown (a judged task that declares no effect,
//!   its trigger a past clock that names no time of its own): no wake at all.
//! - 7: a task due while the instance sleeps: the round's pick is `instance_asleep_wake_due` with
//!   no intent, the plan names it, the planner wakes the instance once (the emulator's start, then
//!   the startup package; a person's self-check stands for the woken instance's preparation), the
//!   task is dispatched, and the instance is stopped again after its runs.
//! - 8: a replay of one idle stretch with a spent-budget task and a benefit-unknown task: one stop,
//!   no start before the release less the lead, one start at it.
//! - 9: ruling 6 in the planner: after a wake that started the emulator, with no run admitted,
//!   the emulator is not stopped again (`auto` takes `close_app` at most, its lighter tiers as
//!   they are) and one warning is due; a run admitted after the wake, a Runtime start, a catalog's
//!   activation and another idle configuration each let the stop be taken again.
//! - 10: a Runtime start with the instance asleep: the first evaluations after it defer the task by
//!   its budget, and the restored planner keeps its wake and starts nothing.
//! - 11: an asleep instance under a person's pause is not woken (`WakeHeld`), its task due behind
//!   the pause; once the pause no longer holds the start the wake comes.
//!
//! Fix round 1 (the coordinator's ruling 7 of the round; review-1859 point 10): two real replays on
//! the same instance with the idle action `stop_emulator`, as the idle host gates configure theirs
//! (`gate_idle1_host`, `gate_idle1p`: its configured emulator start, a startup package, its stuck
//! recovery off, the scripted emulator), with test 5's budget catalogue. No hook makes it asleep:
//! every idle check is the host's own, on the plan its own evaluated cycle leaves
//! (`RuntimeHost::evaluate_policy_cycle`), with its own planner, and the emulator is stopped and
//! started through the claims the product takes.
//! - r1: the host's own check stops the emulator, its wake at the release less the lead; asleep,
//!   the task is deferred by its budget; at the release less the lead the check queues the wake,
//!   whose claim starts the emulator (`drive_autostart`), and its startup package runs; with no run
//!   admitted the checks of the next night do not stop it, and the ledger holds one
//!   `instance_idle_stop_withheld`; after two runs are admitted the check stops it again.
//! - r2: the stop, then a Runtime start in the middle of the sleep (a seeding and a loading start,
//!   the emulator stopped as the action left it, an open on its pending endpoint refused): the
//!   instance is restored stopped with its wake, is prepared by nothing and so is in no unavailable
//!   instances' set, and is asleep (the task deferred by its budget, not `instance_unavailable`) with
//!   no wake for nothing; its wake then starts the emulator, the night's check withholds the stop
//!   once, and a catalogue's activation answers the wake: the next check stops the emulator. The
//!   activated catalogue adds a judged task with no effect on an interval clock, whose benefit is
//!   unknown and whose trigger names a time a millisecond away: on this instance (its idle action
//!   can stop its emulator) that decision names no wake, so the stop is taken with its wake at the
//!   budget's release less the lead (fix round 1, ruling 2).

use super::emulator_controller::stopped_state;
use super::gate_369_s6c::{connectivity, head, send};
use super::gate_372k2::{SlotTask, at, slot_sources};
use super::gate_idle1p::{emulator_starts, kept_rows, stops};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation, IdleWakeCause};
use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, KeptIdle, WakeWork, WithheldStop,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, InstanceStuckRecovery,
    MAX_RUNTIME_SUBSCRIPTION_WAIT_MS, RuntimeConfigManifest, RuntimeLifecyclePhase,
    RuntimeSubscriptionRequest, SchedulingResumeSelfCheck, SubscriptionCursor,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_policy::{EligibilityState, PolicyCode, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::{
    AutoGaps, DefaultsFile, IdleAction, IdleConfig, annotate_settable, mechanism_rows,
    render_parameters,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// The task whose trigger is always true (an interval clock of 1 ms), `daily_limit` 2.
const TASK: &str = "fixture.asleep";
/// A judged task that declares no effect, its trigger a clock that served no occurrence yet.
const JUDGED: &str = "fixture.asleep_judged";
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// `idle.stop_emulator_wake_lead_ms`'s default.
const LEAD_MS: u64 = 300_000;
/// `stop_emulator` with the production gap and leads.
const STOP: IdleConfig = IdleConfig {
    action: IdleAction::StopEmulator,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: LEAD_MS,
    stop_emulator_wake_lead_ms: LEAD_MS,
};
/// What a wake after `stop_emulator` queues, in order.
const STOP_WAKE: IdleStep = IdleStep::Wake {
    action: IdleAction::StopEmulator,
    work: &[WakeWork::EmulatorStart, WakeWork::StartupPackage],
};

// ---- The catalog

/// The budget task: always due, two runs a day.
fn budget_task() -> SlotTask {
    SlotTask {
        id: TASK,
        priority: 100,
        trigger: json!({
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 1,
                "anchor_ms": 0
            }
        }),
        daily_limit: 2,
    }
}

/// The judged task: its trigger a UTC clock `at` 06:05 of day 1 (true once that time has passed,
/// until a run serves it, and naming no later time).
fn judged_task() -> SlotTask {
    SlotTask {
        id: JUDGED,
        priority: 100,
        trigger: json!({
            "kind": "clock",
            "schedule": {
                "kind": "at",
                "clock_source": {
                    "kind": "server",
                    "timezone_id": "etc/utc",
                    "utc_offset_minutes": 0,
                    "dst_offset_minutes": 0,
                    "maintenance_drift_ms": 0
                },
                "at_ms": at(1, 6, 5)
            }
        }),
        daily_limit: 2,
    }
}

/// Fix round 1 (r2): the judged task on an interval clock of 1 ms, so its trigger is true at every
/// evaluation and names its next time a millisecond away.
fn judged_interval_task() -> SlotTask {
    SlotTask {
        trigger: budget_task().trigger,
        ..judged_task()
    }
}

/// `gate_372k2`'s catalog of `tasks` (activity window 06:00-23:30 UTC, activity interval 1 ms),
/// declared `actingcommand.scheduling.v2` (every timeline event valid from 0, as `gate_ss2h`'s V2
/// example), [`JUDGED`] marked `benefit_judged` with no effect.
fn sources(tasks: &[SlotTask]) -> CatalogSources {
    sources_of(1, tasks)
}

/// [`sources`] as the catalog's version `version` (fix round 1: a later version activates).
fn sources_of(version: u64, tasks: &[SlotTask]) -> CatalogSources {
    let mut sources = slot_sources(version, tasks, None);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("a fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if let Some(tasks) = document.get_mut("tasks").and_then(Value::as_array_mut) {
            for task in tasks {
                if task["id"] == json!(JUDGED) {
                    task["benefit_judged"] = json!(true);
                    task["produces"] = json!([]);
                    task["consumes"] = json!([]);
                }
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("a fixture document's bytes");
    }
    sources
}

// ---- The host

/// The fixture instance's scripted device: emulator-controlled from the start (it registers with
/// its emulator running) and its opens passing, so its availability is its self-check's.
fn device_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    for flag in [
        &state.emulator_control,
        &state.observed_open,
        &state.physical_task_geometry,
        &state.transition_capture_after_input,
    ] {
        flag.store(true, Ordering::Release);
    }
    state
}

fn open_host(
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
) -> RuntimeHost {
    // `gate_ord1c`'s fix round 3: each capture of a preparation takes the clock's millisecond, so
    // the start's withdrawal and the seed its passing self-check publishes again are not observed
    // at the same instant.
    let moved = Arc::clone(clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || moved.advance(1)));
    let mut facts = policy_facts();
    facts.outcomes.clear();
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_config_manifest(no_progress_off_manifest(&registered))
            .with_stuck_recovery(BTreeMap::from([(
                ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(FakeProvider::one(ALIAS, registered, Arc::clone(state))),
    )
    .expect("start the fixture's host")
}

/// The fixture instance on a host of its own, on a manual Runtime clock.
struct Sleepy {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    registered: InstanceId,
    approved: BTreeSet<String>,
    root: TempDir,
}

impl Sleepy {
    fn start(sources: &CatalogSources, unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let host = open_host(&root, registered, &clock, &device_state());
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        Self {
            host,
            clock,
            registered,
            approved: BTreeSet::new(),
            root,
        }
    }

    /// A clean stop and a new start on the same state root, at `unix_ms`.
    fn restart(self, unix_ms: u64) -> Self {
        let Self {
            host,
            registered,
            approved,
            root,
            ..
        } = self;
        host.close().expect("close the fixture's host");
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let host = open_host(&root, registered, &clock, &device_state());
        Self {
            host,
            clock,
            registered,
            approved,
            root,
        }
    }

    fn close(self) {
        assert!(
            self.host
                .fatal_error()
                .expect("the Runtime's health")
                .is_none()
        );
        self.host.close().expect("close the fixture's host");
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One cycle at `unix_ms` on the test inputs (no idle check of the host's own).
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    /// Evaluates at `unix_ms`, admits the dispatch the cycle selected for [`TASK`], completes it
    /// and ends its lease, as a run of the task does.
    fn dispatch(&mut self, unix_ms: u64) {
        let cycle = self.evaluate(unix_ms);
        let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK)
            .unwrap_or_else(|| panic!("the task is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit the task at {unix_ms}: {error}"))
        else {
            panic!("the task is granted at {unix_ms}");
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    /// At `unix_ms`, the two ends of `stop_emulator` on the instance: its idle action holds its
    /// emulator stopped (the planner's idle phase after the stop), and the backend self-check gate
    /// withdrew its availability (`backend_selfcheck:unchecked`), as the stop's invalidation of
    /// its self-check facts does.
    fn sleep(&self, unix_ms: u64) {
        self.set_time(unix_ms);
        self.host
            .hold_emulator_stopped_for_test(ALIAS)
            .expect("the idle action holds the emulator stopped");
        self.host
            .publish_backend_selfcheck_unavailable_for_test(ALIAS, "unchecked")
            .expect("the gate withdraws the availability");
    }

    /// At `unix_ms`, a person's self-check of the instance (`actingctl selfcheck`): it passes and
    /// makes the instance available again, as the woken instance's preparation does.
    fn self_check(&self, unix_ms: u64) {
        self.set_time(unix_ms);
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let receipt = self
            .host
            .process_request_for_test(
                &runtime_request(
                    &ids,
                    RuntimeOperation::SelfCheckInstance {
                        instance_alias: ALIAS.to_owned(),
                    },
                ),
                ConnectionId::new(4_861).expect("connection"),
            )
            .expect("the self-check's receipt");
        let Some(RuntimeResult::InstanceSelfChecked {
            selfcheck: SchedulingResumeSelfCheck { failure_code, .. },
            ..
        }) = receipt.result()
        else {
            panic!("a self-check result: {receipt:?}");
        };
        assert_eq!(failure_code, &None, "the self-check passes: {receipt:?}");
    }

    fn scheduling(&self, operation: RuntimeOperation) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(operation);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// A person's instance pause, as `actingctl pause` sends it.
    fn pause(&self) {
        self.scheduling(RuntimeOperation::PauseScheduling {
            scope: actingcommand_contract::SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("asleep_pause")
                .expect("pause reason"),
            drain_timeout_ms: 1_000,
            pause_mode: actingcommand_contract::SchedulingPauseMode::Stand,
        });
    }

    /// The instance's latest plan, as the idle check reads it.
    fn plan(&self) -> IdlePlan {
        self.host
            .idle_plan_and_runs_counted_for_test(ALIAS)
            .expect("the idle plan")
            .0
            .expect("an evaluated plan")
    }

    /// The task the latest plan counts due.
    fn due_task(&self) -> Option<String> {
        self.host
            .idle_due_task_for_test(ALIAS)
            .expect("the plan's due task")
    }

    /// The latest admission of a scheduled run on the instance, as the idle check reads it.
    fn latest_admission(&self) -> Option<u64> {
        self.host
            .idle_latest_admission_for_test(ALIAS)
            .expect("the latest admission")
    }
}

// ---- Reading a cycle

fn decision<'a>(cycle: &'a PolicyCycle, task_id: &str) -> &'a TaskDecision {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .unwrap_or_else(|| panic!("the decision of {task_id}: {evaluation:#?}"))
}

/// The task's decision is eligible and deferred, its last reason of `code`: its wake. Never
/// `instance_unavailable`: the asleep instance is judged as an available one.
fn deferred_by(cycle: &PolicyCycle, task_id: &str, code: &str) -> Option<u64> {
    let decision = decision(cycle, task_id);
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    assert_eq!(
        decision.eligibility,
        EligibilityState::True,
        "{decision:#?}"
    );
    assert_eq!(
        decision
            .reasons
            .last()
            .map(|reason| reason.outcome.code().as_str()),
        Some(code),
        "{decision:#?}"
    );
    assert!(
        !decision
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(PolicyCode::InstanceUnavailable)),
        "{decision:#?}"
    );
    decision.next_wake_unix_ms
}

fn intents(cycle: &PolicyCycle) -> usize {
    cycle
        .evaluation
        .as_ref()
        .expect("an evaluated cycle")
        .dispatch_intents
        .len()
}

// ---- The planner's views

/// The instance free, its emulator running, both actions able to bring it back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

/// The instance free with its emulator stopped by the action.
fn stopped() -> IdleView {
    IdleView {
        emulator_on: false,
        ..free()
    }
}

/// A claim holds the instance's key (a wake's claims: the emulator's start, the startup package).
fn leased() -> IdleView {
    IdleView {
        leased: true,
        ..free()
    }
}

/// A plan evaluated at `at`: a task due, or none and the next one at `next_wake`.
fn plan_at(at: u64, due_now: bool, next_wake: Option<u64>) -> IdlePlan {
    IdlePlan {
        evaluated_at_unix_ms: at,
        due_now,
        next_wake_unix_ms: next_wake,
        reading_due_unix_ms: None,
    }
}

/// The planner after `stop_emulator` on the instance, its wake `wake`.
fn stopped_planner(wake: Option<u64>) -> IdlePlanner {
    let mut planner = IdlePlanner::default();
    planner.restore(
        ALIAS,
        KeptIdle {
            action: IdleAction::StopEmulator,
            wake_at_unix_ms: wake,
            woken: false,
        },
    );
    planner
}

// ---- 5, 6, 7

#[test]
fn asleep_h5_a_spent_budget_does_not_start_the_stopped_emulator_before_its_release() {
    let mut sleepy = Sleepy::start(&sources(&[budget_task()]), at(1, 6, 0));
    sleepy.dispatch(at(1, 6, 10));
    sleepy.dispatch(at(1, 6, 20));

    // Free, the budget defers the task to its release: `stop_emulator` stops the emulator, its
    // wake at the release less the lead.
    let stopped_at = at(1, 6, 30);
    let release = deferred_by(
        &sleepy.evaluate(stopped_at),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the budget names its release");
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(ALIAS, &STOP, free(), Some(sleepy.plan()), stopped_at),
        Some(IdleStep::Act {
            action: IdleAction::StopEmulator,
            next_due_unix_ms: Some(release),
            wake_at_unix_ms: Some(release - LEAD_MS),
        }),
        "the idle check stops the emulator"
    );
    planner.finished(ALIAS, IdleEnd::Done);
    sleepy.sleep(stopped_at + 1_000);

    // Asleep: judged as available, the budget still defers the task to its release; the plan
    // counts it not due and no check starts the emulator again.
    for moment in [stopped_at + 2_000, at(1, 12, 0), at(1, 23, 0)] {
        let cycle = sleepy.evaluate(moment);
        assert_eq!(
            deferred_by(&cycle, TASK, HostCode::PolicyBudgetExhausted.as_str()),
            Some(release),
            "the budget at {moment}"
        );
        let read = sleepy.plan();
        assert!(!read.due_now, "not due at {moment}: {read:?}");
        assert_eq!(read.next_wake_unix_ms, Some(release), "{read:?}");
        assert_eq!(
            planner.check(ALIAS, &STOP, stopped(), Some(read), moment),
            None,
            "no start at {moment}"
        );
        assert_eq!(
            planner.planned_wake(ALIAS),
            Some(release - LEAD_MS),
            "the wake stays the release less the lead"
        );
    }
    sleepy.close();
}

#[test]
fn asleep_h6_a_candidate_whose_benefit_is_unknown_wakes_the_stopped_emulator_never() {
    let sleepy = Sleepy::start(&sources(&[judged_task()]), at(1, 6, 0));
    let stopped_at = at(1, 6, 30);
    assert_eq!(
        deferred_by(
            &sleepy.evaluate(stopped_at),
            JUDGED,
            PolicyCode::BenefitUnknown.as_str()
        ),
        None,
        "an unknown benefit names no wake"
    );
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(ALIAS, &STOP, free(), Some(sleepy.plan()), stopped_at),
        Some(IdleStep::Act {
            action: IdleAction::StopEmulator,
            next_due_unix_ms: None,
            wake_at_unix_ms: None,
        }),
        "the idle check stops the emulator, with no wake"
    );
    planner.finished(ALIAS, IdleEnd::Done);
    sleepy.sleep(stopped_at + 1_000);

    for moment in [stopped_at + 2_000, at(1, 18, 0), at(2, 6, 10)] {
        let cycle = sleepy.evaluate(moment);
        assert_eq!(
            deferred_by(&cycle, JUDGED, PolicyCode::BenefitUnknown.as_str()),
            None,
            "the benefit gate holds it at {moment}, with no wake"
        );
        let read = sleepy.plan();
        assert!(!read.due_now, "not due at {moment}: {read:?}");
        assert_eq!(read.next_wake_unix_ms, None, "no wake at {moment}");
        assert_eq!(
            planner.check(ALIAS, &STOP, stopped(), Some(read), moment),
            None,
            "no start at {moment}"
        );
        assert_eq!(planner.planned_wake(ALIAS), None, "no wake at all");
    }
    sleepy.close();
}

#[test]
fn asleep_h7_a_task_due_while_asleep_starts_the_emulator_once_runs_and_sleeps_again() {
    let mut sleepy = Sleepy::start(&sources(&[budget_task()]), at(1, 6, 0));
    sleepy.dispatch(at(1, 6, 10));
    sleepy.dispatch(at(1, 6, 20));
    let stopped_at = at(1, 6, 30);
    let release = deferred_by(
        &sleepy.evaluate(stopped_at),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the budget names its release");
    let mut planner = IdlePlanner::default();
    assert!(
        matches!(
            planner.check(ALIAS, &STOP, free(), Some(sleepy.plan()), stopped_at),
            Some(IdleStep::Act {
                action: IdleAction::StopEmulator,
                ..
            })
        ),
        "the idle check stops the emulator"
    );
    planner.finished(ALIAS, IdleEnd::Done);
    sleepy.sleep(stopped_at + 1_000);

    // The next day, past the release: the round's pick on the asleep instance is the task it is
    // woken for, at the evaluation's time, with no intent.
    let due_at = at(2, 6, 10);
    assert!(
        release <= due_at,
        "the release {release} comes before {due_at}"
    );
    let cycle = sleepy.evaluate(due_at);
    assert_eq!(
        deferred_by(&cycle, TASK, PolicyCode::InstanceAsleepWakeDue.as_str()),
        Some(due_at),
        "its wake is the evaluation's time"
    );
    assert_eq!(
        intents(&cycle),
        0,
        "nothing is dispatched on an asleep instance"
    );
    let read = sleepy.plan();
    assert!(read.due_now, "{read:?}");
    assert_eq!(
        sleepy.due_task(),
        Some(TASK.to_owned()),
        "the plan names it"
    );
    let woken_with = sleepy.latest_admission();
    assert_eq!(
        planner.check_with_due_task(
            ALIAS,
            &STOP,
            IdleView {
                latest_admission: woken_with,
                ..stopped()
            },
            Some(read),
            due_at,
            Some(TASK),
        ),
        Some(STOP_WAKE),
        "the emulator's start, then the startup package"
    );
    assert_eq!(planner.woken_by(ALIAS), Some(IdleWakeCause::TaskDue));
    // The wake's configured start drives the emulator's Start (the host's autostart claim says
    // so to the planner).
    planner.wake_started_emulator(ALIAS);

    // The wake's claims hold the instance: no second start.
    let held_at = due_at + MINUTE_MS;
    sleepy.evaluate(held_at);
    assert_eq!(
        planner.check(
            ALIAS,
            &STOP,
            IdleView {
                latest_admission: woken_with,
                ..leased()
            },
            Some(sleepy.plan()),
            held_at,
        ),
        None,
        "the emulator is started once"
    );

    // Prepared again, the task is dispatched (its two runs of the day), and the free instance is
    // stopped again afterwards.
    sleepy.self_check(due_at + 2 * MINUTE_MS);
    sleepy.dispatch(due_at + 3 * MINUTE_MS);
    sleepy.dispatch(due_at + 4 * MINUTE_MS);
    let after = due_at + 10 * MINUTE_MS;
    let next_release = deferred_by(
        &sleepy.evaluate(after),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the day's budget names its release");
    let ran = sleepy.latest_admission();
    assert!(
        ran.is_some_and(|ran| woken_with.is_none_or(|woken| ran > woken)),
        "a run admitted after the wake: {ran:?} after {woken_with:?}"
    );
    assert_eq!(
        planner.check(
            ALIAS,
            &STOP,
            IdleView {
                latest_admission: ran,
                ..free()
            },
            Some(sleepy.plan()),
            after,
        ),
        Some(IdleStep::Act {
            action: IdleAction::StopEmulator,
            next_due_unix_ms: Some(next_release),
            wake_at_unix_ms: Some(next_release - LEAD_MS),
        }),
        "the instance sleeps again"
    );
    assert_eq!(planner.take_withheld_stop(ALIAS), None, "no warning");
    sleepy.close();
}

// ---- 8

#[test]
fn asleep_h8_one_idle_stretch_one_stop_and_one_start_at_the_release() {
    let mut sleepy = Sleepy::start(&sources(&[budget_task(), judged_task()]), at(1, 6, 0));
    sleepy.dispatch(at(1, 6, 10));
    sleepy.dispatch(at(1, 6, 20));
    let stopped_at = at(1, 6, 30);
    let cycle = sleepy.evaluate(stopped_at);
    let release = deferred_by(&cycle, TASK, HostCode::PolicyBudgetExhausted.as_str())
        .expect("the budget names its release");
    deferred_by(&cycle, JUDGED, PolicyCode::BenefitUnknown.as_str());
    let mut planner = IdlePlanner::default();
    let mut stops = 0;
    let mut starts = 0;
    let mut count = |step: Option<IdleStep>| match step {
        Some(IdleStep::Act { .. }) => stops += 1,
        Some(IdleStep::Wake { .. }) => starts += 1,
        _ => {}
    };
    count(planner.check(ALIAS, &STOP, free(), Some(sleepy.plan()), stopped_at));
    planner.finished(ALIAS, IdleEnd::Done);
    sleepy.sleep(stopped_at + 1_000);
    let wake = release - LEAD_MS;
    for moment in [
        stopped_at + 2_000,
        at(1, 8, 0),
        at(1, 12, 0),
        at(1, 18, 0),
        at(1, 23, 0),
        wake - 1,
    ] {
        sleepy.evaluate(moment);
        let read = sleepy.plan();
        assert!(!read.due_now, "not due at {moment}: {read:?}");
        count(planner.check(ALIAS, &STOP, stopped(), Some(read), moment));
    }
    sleepy.evaluate(wake);
    count(planner.check(ALIAS, &STOP, stopped(), Some(sleepy.plan()), wake));
    assert_eq!(
        planner.woken_by(ALIAS),
        Some(IdleWakeCause::PlannedTimeReached)
    );
    assert_eq!(
        (stops, starts),
        (1, 1),
        "one stop, no start before the release less the lead, one start at it"
    );
    sleepy.close();
}

// ---- 9

#[test]
fn asleep_h9_a_wake_no_run_answers_keeps_the_emulator_running_and_warns_once() {
    let woken_at = at(1, 6, 0);
    let far = at(1, 12, 0);
    // A wake for a due task whose configured start started the emulator (`started`; the host's
    // autostart claim says so when it drove the Start), then its claims, then the instance free
    // with nothing due before `far`: no run was admitted since the wake.
    let woken_and = |config: &IdleConfig, started: bool| {
        let mut planner = stopped_planner(None);
        assert_eq!(
            planner.check_with_due_task(
                ALIAS,
                config,
                stopped(),
                Some(plan_at(woken_at, true, None)),
                woken_at,
                Some(TASK),
            ),
            Some(STOP_WAKE)
        );
        if started {
            planner.wake_started_emulator(ALIAS);
        }
        let held = woken_at + MINUTE_MS;
        assert_eq!(
            planner.check(
                ALIAS,
                config,
                leased(),
                Some(plan_at(held, true, None)),
                held
            ),
            None
        );
        planner
    };
    let woken = |config: &IdleConfig| woken_and(config, true);
    let free_at = |minutes: u64| woken_at + minutes * MINUTE_MS;

    // A wake whose start found the emulator running started nothing: the stop is taken as before.
    let mut found_running = woken_and(&STOP, false);
    let checked = free_at(10);
    assert!(
        matches!(
            found_running.check(
                ALIAS,
                &STOP,
                free(),
                Some(plan_at(checked, false, Some(far))),
                checked
            ),
            Some(IdleStep::Act {
                action: IdleAction::StopEmulator,
                ..
            })
        ),
        "a wake that started nothing binds no stop"
    );
    assert_eq!(found_running.take_withheld_stop(ALIAS), None);

    // `stop_emulator`: not stopped again, one warning; not again.
    let mut planner = woken(&STOP);
    let first = free_at(10);
    assert_eq!(
        planner.check(
            ALIAS,
            &STOP,
            free(),
            Some(plan_at(first, false, Some(far))),
            first
        ),
        None,
        "the emulator is not stopped again"
    );
    assert_eq!(
        planner.take_withheld_stop(ALIAS),
        Some(WithheldStop {
            cause: IdleWakeCause::TaskDue,
            task_id: Some(TASK.to_owned()),
            woken_at_unix_ms: woken_at,
        }),
        "one warning, with the wake's cause, task and time"
    );
    let second = free_at(20);
    assert_eq!(
        planner.check(
            ALIAS,
            &STOP,
            free(),
            Some(plan_at(second, false, Some(far))),
            second
        ),
        None
    );
    assert_eq!(planner.take_withheld_stop(ALIAS), None, "once per wake");

    // A scheduled run admitted after the wake: the stop is taken again.
    let ran = free_at(30);
    assert!(
        matches!(
            planner.check(
                ALIAS,
                &STOP,
                IdleView {
                    latest_admission: Some(7),
                    ..free()
                },
                Some(plan_at(ran, false, Some(far))),
                ran
            ),
            Some(IdleStep::Act {
                action: IdleAction::StopEmulator,
                ..
            })
        ),
        "an admitted run answers the wake"
    );

    // `auto`: the tier is capped below the emulator's stop, the lighter tiers as they are.
    let auto = IdleConfig {
        action: IdleAction::Auto(AutoGaps {
            return_home_min_gap_ms: 10 * MINUTE_MS,
            stop_emulator_min_gap_ms: 2 * HOUR_MS,
        }),
        ..STOP
    };
    let mut capped = woken(&auto);
    assert_eq!(
        capped.check(
            ALIAS,
            &auto,
            free(),
            Some(plan_at(first, false, Some(far))),
            first
        ),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(far),
            wake_at_unix_ms: Some(far - LEAD_MS),
        }),
        "auto takes close_app instead of the emulator's stop"
    );
    assert!(
        capped.take_withheld_stop(ALIAS).is_some(),
        "the stop not taken is warned"
    );
    let mut lighter = woken(&auto);
    let near = first + HOUR_MS;
    assert_eq!(
        lighter.check(
            ALIAS,
            &auto,
            free(),
            Some(plan_at(first, false, Some(near))),
            first
        ),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: Some(near),
            wake_at_unix_ms: Some(near - LEAD_MS),
        }),
        "a gap of one hour takes close_app as before"
    );
    assert_eq!(
        lighter.take_withheld_stop(ALIAS),
        None,
        "no stop was withheld"
    );

    // A catalog's activation and another idle configuration each answer the wake; a Runtime
    // start keeps none (its planner restores no memory of a wake).
    let mut activated = woken(&STOP);
    activated.forget_unanswered_wakes();
    let changed = IdleConfig {
        stop_emulator_wake_lead_ms: 2 * LEAD_MS,
        ..STOP
    };
    let mut reconfigured = woken(&STOP);
    for (case, planner, config) in [
        ("an activation", &mut activated, &STOP),
        ("an idle change", &mut reconfigured, &changed),
        ("a Runtime start", &mut IdlePlanner::default(), &STOP),
    ] {
        assert!(
            matches!(
                planner.check(
                    ALIAS,
                    config,
                    free(),
                    Some(plan_at(first, false, Some(far))),
                    first
                ),
                Some(IdleStep::Act {
                    action: IdleAction::StopEmulator,
                    ..
                })
            ),
            "{case}: the stop is taken"
        );
        assert_eq!(planner.take_withheld_stop(ALIAS), None, "{case}");
    }
}

// ---- 10, 11

#[test]
fn asleep_h10_a_start_keeps_the_asleep_instance_asleep_with_its_wake() {
    let mut sleepy = Sleepy::start(&sources(&[budget_task()]), at(1, 6, 0));
    sleepy.dispatch(at(1, 6, 10));
    sleepy.dispatch(at(1, 6, 20));
    let release = deferred_by(
        &sleepy.evaluate(at(1, 6, 30)),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the budget names its release");
    sleepy.sleep(at(1, 6, 31));
    // The root's second start seeds (it replays the history), its third loads the decision memory
    // from its value records, as every restart of an installation does. The start restores the
    // closed instance from `host.idle_state` and withholds its availability; the fixture's device
    // passes its start's self-check, so the two ends of the stop stand for what it restores.
    let sleepy = sleepy.restart(at(1, 7, 0));
    let sleepy = sleepy.restart(at(1, 7, 1));
    let started = at(1, 7, 1) + 1_000;
    sleepy.sleep(started);
    let mut planner = stopped_planner(Some(release - LEAD_MS));
    assert_eq!(
        planner.check(ALIAS, &STOP, stopped(), None, started),
        None,
        "the start's first check, before any evaluation"
    );
    for moment in [started + 1_000, started + 2_000] {
        let cycle = sleepy.evaluate(moment);
        assert_eq!(
            deferred_by(&cycle, TASK, HostCode::PolicyBudgetExhausted.as_str()),
            Some(release),
            "the budget at {moment}"
        );
        let read = sleepy.plan();
        assert!(!read.due_now, "{read:?}");
        assert_eq!(
            planner.check(ALIAS, &STOP, stopped(), Some(read), moment),
            None,
            "no wake for nothing at {moment}"
        );
        assert_eq!(planner.planned_wake(ALIAS), Some(release - LEAD_MS));
    }
    sleepy.close();
}

#[test]
fn asleep_h11_an_asleep_instance_under_a_persons_pause_is_not_woken() {
    let sleepy = Sleepy::start(&sources(&[budget_task()]), at(1, 6, 0));
    sleepy.sleep(at(1, 6, 1));
    sleepy.pause();
    let paused_at = at(1, 6, 2);
    let cycle = sleepy.evaluate(paused_at);
    assert_eq!(
        deferred_by(&cycle, TASK, HostCode::DispatchPausedInstance.as_str()),
        None,
        "the pause holds the task with no wake"
    );
    let read = sleepy.plan();
    assert!(
        read.due_now,
        "a task no later gate holds back is due behind the pause: {read:?}"
    );
    let mut planner = stopped_planner(None);
    let held = IdleView {
        paused: true,
        start_held: true,
        ..stopped()
    };
    assert_eq!(
        planner.check(ALIAS, &STOP, held, Some(read), paused_at),
        Some(IdleStep::WakeHeld),
        "not woken under the pause"
    );
    assert_eq!(planner.woken_by(ALIAS), None);
    let resumed_at = paused_at + MINUTE_MS;
    assert_eq!(
        planner.check(ALIAS, &STOP, stopped(), Some(read), resumed_at),
        Some(STOP_WAKE),
        "woken once the pause no longer holds the start"
    );
    sleepy.close();
}

// ---- Fix round 1: the real replays (ruling 7)

/// Every bounded wait of the replays (test plan v2.2 §2.1 guard 3: a gate file waits only with the
/// constant of this name); it only bounds a broken run.
const GATE_WAIT: Duration = Duration::from_secs(60);

/// One idle record of the replay instance: its code, its `operation` and its `idle_wake_cause`.
type IdleRecord = (String, String, Option<String>);

/// The stop's record (`instance_idle_action`, `run_idle_action`).
fn stop_record() -> IdleRecord {
    (
        IdleCode::Taken.as_str().to_owned(),
        IdleLocation::Action.as_str().to_owned(),
        None,
    )
}

/// The record of a wake at its planned time (`instance_idle_action`, `queue_idle_wake`,
/// `planned_time_reached`).
fn wake_record() -> IdleRecord {
    (
        IdleCode::Taken.as_str().to_owned(),
        IdleLocation::Wake.as_str().to_owned(),
        Some(IdleWakeCause::PlannedTimeReached.as_str().to_owned()),
    )
}

/// The one warning of that wake (`instance_idle_stop_withheld`, `check_idle_instances`, its cause).
fn withheld_record() -> IdleRecord {
    (
        IdleCode::StopWithheld.as_str().to_owned(),
        IdleLocation::Check.as_str().to_owned(),
        Some(IdleWakeCause::PlannedTimeReached.as_str().to_owned()),
    )
}

/// The replay instance's row of `host.idle_state`: stopped by `stop_emulator`, its wake at `wake`.
fn stopped_row(wake: u64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        (
            "instance_alias".to_owned(),
            FactScalar::String(ALIAS.to_owned()),
        ),
        (
            "action".to_owned(),
            FactScalar::String(IdleAction::StopEmulator.as_str().to_owned()),
        ),
        ("wake_at_unix_ms".to_owned(), FactScalar::TimestampMs(wake)),
    ])
}

/// The replay instance's configuration, built as `gate_484_idle_w2`'s `close_app` fixture builds
/// its own: the `idle` section's keys as the assembly renders them from the shipped defaults file,
/// the instance's own `idle.action` `stop_emulator` and `start_emulator` (the configuration's check
/// asks for it beside `stop_emulator`), and its no-progress guard off.
fn stop_emulator_manifest(instance: InstanceId) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let section = actingcommand_runtime_config::idle::SECTION;
    let id = crate::failure_identity::identifier_text(&instance);
    let own = |member: &str, value: FactScalar| ConfigParameter {
        key: actingcommand_runtime_config::instance_key(&id, member),
        value,
        source: ConfigParameterSource::Explicit,
        settable: false,
    };
    let mut parameters =
        render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
            .expect("every key resolves")
            .into_iter()
            .filter(|row| row.key.split('.').next() == Some(section))
            .collect::<Vec<_>>();
    parameters.push(own(
        actingcommand_runtime_config::idle::ACTION,
        FactScalar::String(IdleAction::StopEmulator.as_str().to_owned()),
    ));
    parameters.push(own(
        actingcommand_runtime_config::START_EMULATOR,
        FactScalar::Boolean(true),
    ));
    parameters.extend(no_progress_off_manifest(&instance).parameters);
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A start of the replay instance's host on `root`, on `clock`: the fixture instance of tests 5 to
/// 11 (emulator-controlled and device self-checked, `device_state`) with the idle action
/// `stop_emulator`, its configured emulator start in effect and in the start turn
/// (`with_emulator_autostart`), a startup package, its stuck recovery off and internet access at
/// once, as the idle host gates configure their instance.
fn open_replay_host(
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
) -> RuntimeHost {
    // `gate_ord1c`'s fix round 3, as `open_host` above.
    let moved = Arc::clone(clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || moved.advance(1)));
    let package = root.path().join("asleep-startup-package.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write the startup package");
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("the startup package request");
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let mut facts = policy_facts();
    facts.outcomes.clear();
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_config_manifest(stop_emulator_manifest(registered))
            .with_stuck_recovery(BTreeMap::from([(
                ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )]))
            .with_emulator_autostart(vec![ALIAS.to_owned()])
            .with_network_connectivity_for_test(read)
            .with_startup_packages(BTreeMap::from([(ALIAS.to_owned(), startup)])),
        Arc::new(FakeProvider::one(ALIAS, registered, Arc::clone(state))),
    )
    .expect("start the replay's host")
}

/// The replay instance on a host of its own with test 5's budget catalogue. Every idle check is
/// the host's own, on the plan its own evaluated cycle leaves, with its own planner.
struct Replay {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    registered: InstanceId,
    approved: BTreeSet<String>,
    root: TempDir,
}

impl Replay {
    fn start(unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let state = device_state();
        let host = open_replay_host(&root, registered, &clock, &state);
        host.activate_policy_catalog(&sources(&[budget_task()]))
            .expect("activate the budget catalogue");
        Self {
            host,
            clock,
            state,
            registered,
            approved: BTreeSet::new(),
            root,
        }
    }

    /// A clean stop and a new start on the same state root at `unix_ms`, its device's emulator
    /// stopped as the idle action left it: no port reported, and an open on its pending endpoint
    /// refused, as the device registry refuses one (`refuse_pending_opens`).
    fn restart(self, unix_ms: u64) -> Self {
        let Self {
            host,
            registered,
            approved,
            root,
            ..
        } = self;
        host.close().expect("close the replay's host");
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let state = device_state();
        state.emulator.rebind(None);
        state.refuse_pending_opens.store(true, Ordering::Release);
        let host = open_replay_host(&root, registered, &clock, &state);
        Self {
            host,
            clock,
            state,
            registered,
            approved,
            root,
        }
    }

    fn close(self) {
        assert!(
            self.host
                .fatal_error()
                .expect("the Runtime's health")
                .is_none()
        );
        self.host.close().expect("close the replay's host");
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One evaluation at `unix_ms` on the test inputs, with no idle check.
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    /// One cycle at `unix_ms` as actingd's driver asks for it: the evaluation, then the host's own
    /// idle check on the plan it leaves (`gate_484_idle_w2`'s `OwnHost::cycle`).
    fn cycle(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let cycle = self
            .host
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("the cycle at {unix_ms}: {error}"));
        assert!(
            cycle.evaluation.is_some(),
            "the cycle at {unix_ms} is evaluated: {:?}",
            cycle.directive
        );
        cycle
    }

    /// Evaluates at `unix_ms`, admits the dispatch the cycle selected for [`TASK`], completes it
    /// and ends its lease, as a run of the task does ([`Sleepy::dispatch`]).
    fn dispatch(&mut self, unix_ms: u64) {
        let cycle = self.evaluate(unix_ms);
        let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK)
            .unwrap_or_else(|| panic!("the task is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit the task at {unix_ms}: {error}"))
        else {
            panic!("the task is granted at {unix_ms}");
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    /// The instance's idle records after `after`, in order: `instance_idle_action` and
    /// `instance_idle_stop_withheld`.
    fn records(&self, after: u64) -> Vec<IdleRecord> {
        let events = all_events(&self.host);
        phases(&events)
            .into_iter()
            .filter(|(event, _)| {
                event.sequence() > after && event.links().instance_id() == Some(&self.registered)
            })
            .filter_map(|(_, phase)| {
                let RuntimeLifecyclePhase::DeviceStateObserved { outcome } = phase else {
                    return None;
                };
                let code = outcome.code().as_str();
                (code == IdleCode::Taken.as_str() || code == IdleCode::StopWithheld.as_str()).then(
                    || {
                        let text = |key: &str| {
                            outcome
                                .value(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        };
                        (
                            code.to_owned(),
                            text("operation").unwrap_or_default(),
                            text("idle_wake_cause"),
                        )
                    },
                )
            })
            .collect()
    }

    /// Whether nothing holds or waits for the instance's key: no idle action's claim was queued.
    fn claims_free(&self) -> bool {
        self.host
            .instance_claims_for_test(ALIAS)
            .expect("the instance's claims")
            == (None, Vec::new())
    }

    /// Waits until the configured start the wake queued has ended (one of its report lines) and
    /// returns the code of that line.
    fn await_start_end(&self) -> String {
        let ends = [
            HostCode::EmulatorAutostartStarted,
            HostCode::EmulatorAutostartPreparationFailed,
            HostCode::EmulatorAutostartFailed,
            HostCode::EmulatorAutostartSkipped,
        ];
        let end = |line: &Outcome| ends.iter().any(|code| line.is(*code));
        let lines = self
            .host
            .emulator_autostart_reports()
            .await_lines_for_test(|lines| lines.iter().any(end), GATE_WAIT)
            .expect("the wake's configured start ended");
        lines
            .iter()
            .find(|&line| end(line))
            .map(|line| line.code().as_str().to_owned())
            .expect("the line that ended it")
    }

    /// Waits until nothing holds or waits for the instance's key: the wake's claims have ended, its
    /// startup package's run among them. Each wait is a ledger subscription that answers as soon as
    /// the instance's next key release after `after` is written (`gate_idle1p`'s `await_release`),
    /// so `GATE_WAIT` only bounds a broken run; the admission guard the last release held is then
    /// taken and let go.
    fn await_free(&self, mut after: u64) {
        let empty_answers = GATE_WAIT
            .as_millis()
            .div_ceil(u128::from(MAX_RUNTIME_SUBSCRIPTION_WAIT_MS));
        let mut empty = 0;
        while !self.claims_free() {
            let query = EventQuery {
                event_type: Some(EventType::LeaseReleased),
                instance_id: Some(self.registered),
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
                &self.host,
                RuntimeOperation::SubscribeEvents {
                    request: subscription,
                },
                4_862,
            );
            let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
                panic!("an event batch: {receipt:?}");
            };
            if batch.timed_out() {
                empty += 1;
                assert!(empty < empty_answers, "the instance's key is released");
                continue;
            }
            empty = 0;
            after = self
                .host
                .query_persisted_events_for_test(query)
                .expect("the instance's key releases")
                .first()
                .map(PersistedEvent::sequence)
                .expect("the instance's next key release");
        }
        let admission = self
            .host
            .instance_admission_for_test(ALIAS)
            .expect("the instance's admission guard");
        drop(admission.lock().expect("the instance's admission guard"));
    }
}

#[test]
fn asleep_r1_the_hosts_own_check_stops_wakes_and_keeps_the_emulator_until_a_run_is_admitted() {
    let mut replay = Replay::start(at(1, 6, 0));
    let before = head(&replay.host);
    // The day's two runs spend the task's budget.
    replay.dispatch(at(1, 6, 10));
    replay.dispatch(at(1, 6, 20));

    // (i) 06:30, free: the budget defers the task to its release, and the host's own check stops
    // the emulator, its wake at the release less the lead.
    let release = deferred_by(
        &replay.cycle(at(1, 6, 30)),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the budget names its release");
    assert_eq!(
        replay.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the stop ended"
    );
    assert_eq!(
        stops(&replay.state),
        1,
        "the host's own check stopped the emulator"
    );
    let wake = release - LEAD_MS;
    assert_eq!(
        kept_rows(&replay.host),
        Some(vec![stopped_row(wake)]),
        "kept stopped, its wake at the release less the lead"
    );

    // Asleep: the evaluation judges the instance as an available one (the budget defers the task,
    // not `instance_unavailable`), and the check starts nothing.
    assert_eq!(
        deferred_by(
            &replay.cycle(at(1, 12, 0)),
            TASK,
            HostCode::PolicyBudgetExhausted.as_str()
        ),
        Some(release),
        "asleep, the budget defers the task to its release"
    );
    assert_eq!(
        replay.records(before),
        [stop_record()],
        "no wake before its time"
    );

    // (ii) The release less the lead: the check queues the wake. (iii) Its claim reads the
    // emulator stopped and starts it (`drive_autostart`); its startup package runs.
    replay.state.emulator.script_read(stopped_state());
    let woken_at = head(&replay.host);
    replay.cycle(wake);
    assert_eq!(
        replay.records(before),
        [stop_record(), wake_record()],
        "the wake at its planned time"
    );
    assert_eq!(
        replay.await_start_end(),
        HostCode::EmulatorAutostartStarted.as_str(),
        "the wake's claim started the emulator"
    );
    assert_eq!(emulator_starts(&replay.state), 1, "one Start");
    replay.await_free(woken_at);

    // (iv) No run admitted: the next night's checks (the task's window closed, its next work at
    // the window's opening, hours away) do not stop the emulator the wake started, and the ledger
    // holds one warning for that wake.
    for moment in [at(3, 1, 0), at(3, 1, 10)] {
        replay.cycle(moment);
        assert!(
            replay.claims_free(),
            "the check at {moment} took no idle action"
        );
    }
    assert_eq!(stops(&replay.state), 1, "not stopped again");
    assert_eq!(
        replay.records(before),
        [stop_record(), wake_record(), withheld_record()],
        "one instance_idle_stop_withheld"
    );

    // (v) The day's two runs admitted: the check stops the emulator again, with no second warning.
    replay.dispatch(at(3, 6, 10));
    replay.dispatch(at(3, 6, 20));
    replay.cycle(at(3, 6, 30));
    assert_eq!(
        replay.host.await_idle_action_ends_for_test(2, GATE_WAIT),
        Some(2),
        "the second stop ended"
    );
    assert_eq!(
        stops(&replay.state),
        2,
        "a run admitted after the wake answers it"
    );
    assert_eq!(
        replay.records(before),
        [
            stop_record(),
            wake_record(),
            withheld_record(),
            stop_record()
        ],
        "the stop again"
    );
    replay.close();
}

#[test]
fn asleep_r2_a_start_keeps_the_instance_asleep_and_an_activation_answers_the_wake() {
    let mut replay = Replay::start(at(1, 6, 0));
    replay.dispatch(at(1, 6, 10));
    replay.dispatch(at(1, 6, 20));
    let release = deferred_by(
        &replay.cycle(at(1, 6, 30)),
        TASK,
        HostCode::PolicyBudgetExhausted.as_str(),
    )
    .expect("the budget names its release");
    assert_eq!(
        replay.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the stop ended"
    );
    let wake = release - LEAD_MS;
    let kept = Some(vec![stopped_row(wake)]);
    assert_eq!(kept_rows(&replay.host), kept, "kept stopped with its wake");

    // Ruling 5, a Runtime start in the middle of the sleep: the root's second start seeds (it
    // replays the history), its third loads the decision memory from its value records, as every
    // restart of an installation does. The emulator stays stopped as the idle action left it.
    let replay = replay.restart(at(1, 7, 0));
    let replay = replay.restart(at(1, 7, 1));
    let started = head(&replay.host);
    assert_eq!(
        kept_rows(&replay.host),
        kept,
        "the start restored the instance stopped with its wake"
    );
    assert_eq!(
        replay
            .host
            .unavailable_for_test(ALIAS)
            .expect("the unavailable instances"),
        None,
        "the start prepared nothing on the stopped emulator, so no failed preparation took it in"
    );
    assert_eq!(
        deferred_by(
            &replay.cycle(at(1, 8, 0)),
            TASK,
            HostCode::PolicyBudgetExhausted.as_str()
        ),
        Some(release),
        "asleep after the start: the budget defers the task, not instance_unavailable"
    );
    assert!(
        replay.records(started).is_empty(),
        "no wake for nothing after the start"
    );
    assert_eq!(emulator_starts(&replay.state), 0, "nothing starts it");

    // Its wake starts the emulator; the night's check withholds the stop, once.
    replay.state.emulator.script_read(stopped_state());
    let woken_at = head(&replay.host);
    replay.cycle(wake);
    assert_eq!(replay.records(started), [wake_record()], "the wake");
    assert_eq!(
        replay.await_start_end(),
        HostCode::EmulatorAutostartStarted.as_str(),
        "the wake's claim started the emulator"
    );
    assert_eq!(emulator_starts(&replay.state), 1, "one Start");
    replay.await_free(woken_at);
    replay.cycle(at(3, 1, 0));
    assert!(replay.claims_free(), "the check took no idle action");
    assert_eq!(stops(&replay.state), 0, "not stopped");
    assert_eq!(
        replay.records(started),
        [wake_record(), withheld_record()],
        "one instance_idle_stop_withheld"
    );

    // A catalogue's activation answers the wake: the next check stops the emulator. The activated
    // catalogue adds a judged task on an interval clock whose benefit is unknown: on this instance
    // (its idle action can stop its emulator) its decision names no wake in the idle plan (fix
    // round 1, ruling 2), so the stop is taken and its wake is the budget's release less the lead.
    replay
        .host
        .activate_policy_catalog(&sources_of(2, &[budget_task(), judged_interval_task()]))
        .expect("activate the second catalogue");
    let cycle = replay.cycle(at(3, 1, 10));
    let judged = decision(&cycle, JUDGED);
    assert!(
        judged.reasons.last().is_some_and(|reason| {
            reason.outcome.is(PolicyCode::BenefitUnknown)
                || reason.outcome.is(PolicyCode::PolicyBenefitNonpositive)
        }),
        "the benefit gate holds the judged task: {judged:#?}"
    );
    assert_eq!(
        replay.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the stop after the activation ended"
    );
    assert_eq!(stops(&replay.state), 1, "the activation answered the wake");
    assert_eq!(
        kept_rows(&replay.host),
        Some(vec![stopped_row(at(3, 6, 0) - LEAD_MS)]),
        "its wake at the window's opening less the lead, none from the judged task"
    );
    assert_eq!(
        replay.records(started),
        [wake_record(), withheld_record(), stop_record()],
        "one warning, then the stop"
    );
    replay.close();
}
