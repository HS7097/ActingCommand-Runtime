// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #405 D1 (host gate key 122; register §1.135; `contracts/scheduling-blackout.md`,
//! "Order of the deferrals", position 9): once an instance has its random delay enabled, every
//! task the scheduler dispatches there first waits a random time. Alice (2026-10-14): the delay is
//! the scheduler's and covers every task of the instance; the coordinator's decisions Q6 to Q12
//! of 2026-10-19 say what is due, what is kept and what is cut.
//!
//! A neutral fixture: one instance (the fixture alias) whose own
//! `instance.<id>.scheduling.random_delay.*` keys the test sets; generic tasks `task_a`, `task_b`
//! and `task_c` with UTC daily clocks, a neutral readiness fact or a dependency; the clock-slot
//! fixture's catalog (activity window 06:00–23:30 UTC, an activity interval of 1 ms); a manual
//! Runtime clock. Every expected value is the brief's requirement or the coordinator's decision.

use super::gate_372k2::{SlotTask, at, slot_rows, slot_sources};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, PolicyRandomDelay,
    RuntimeConfigManifest,
};
use actingcommand_policy::{PolicyEvaluation, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::instance_key;
use actingcommand_runtime_config::scheduling::{
    RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS,
};
use serde_json::{Value, json};

const GATE_WAIT: Duration = Duration::from_secs(30);
const MINUTE_MS: u64 = 60_000;
const SEED: u64 = 7;
const TASK_A: &str = "task_a";
const TASK_B: &str = "task_b";
const TASK_C: &str = "task_c";
const READY_KEY: &str = "resource.neutral.slot_ready";
// The delay bounds of Alice's ruling: at least one minute, at most one hour.
const MIN_MS: u64 = 60_000;
const MAX_MS: u64 = 3_600_000;
const NOON_MINUTE: u16 = 12 * 60;
const NOON: u64 = at(0, 12, 0);
// The fixture window's end, 23:30 UTC, and its last millisecond.
const WINDOW_END: u64 = at(0, 23, 30);
const WINDOW_LAST: u64 = WINDOW_END - 1;

// ---- The catalog and the facts.

/// A UTC daily clock at `minute` of the day.
fn daily(minute: u16) -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": [minute]
        }
    })
}

/// True while the instance's neutral readiness fact is `true` (and, with `max_age_ms`, no older
/// than that).
fn ready_trigger(max_age_ms: Option<u64>) -> Value {
    json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": READY_KEY,
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": max_age_ms
    })
}

/// True once `task_id` succeeded on the instance.
fn dependency(task_id: &str) -> Value {
    json!({"kind": "dependency_completed", "task_id": task_id, "terminal_states": ["succeeded"]})
}

fn task(id: &'static str, priority: u32, trigger: Value) -> SlotTask {
    SlotTask {
        id,
        priority,
        trigger,
        daily_limit: 8,
    }
}

/// The fixture's facts with no scheduling outcome, and the readiness fact `(value, observed at)`
/// when one is given.
fn facts(ready: Option<(bool, u64)>) -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    if let Some((value, observed_at_unix_ms)) = ready {
        facts.facts.push(ObservedFact {
            scope: ScopeSelector::Instance {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            fact_key: READY_KEY.to_owned(),
            value: FactValue::Boolean(value),
            observed_at_unix_ms,
            expires_at_unix_ms: None,
            confidence_milli: 1_000,
        });
    }
    facts
}

/// A configuration whose instance `instance` sets its own random delay.
fn delay_manifest(
    instance: InstanceId,
    enabled: bool,
    min_ms: u64,
    max_ms: u64,
) -> RuntimeConfigManifest {
    let id = crate::failure_identity::identifier_text(&instance);
    let own = |member: &str, value: FactScalar| ConfigParameter {
        key: instance_key(&id, member),
        value,
        source: ConfigParameterSource::Explicit,
        settable: false,
    };
    let mut parameters = vec![
        own(RANDOM_DELAY_ENABLED, FactScalar::Boolean(enabled)),
        own(RANDOM_DELAY_MIN_MS, FactScalar::DurationMs(min_ms)),
        own(RANDOM_DELAY_MAX_MS, FactScalar::DurationMs(max_ms)),
    ];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// ---- What an evaluation says.

/// A pair's `dispatch_random_delay_waiting` reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiting {
    due_at_unix_ms: u64,
    random_delay_ms: u64,
    planned_start_unix_ms: u64,
}

impl Waiting {
    /// The `random_delay` member the pair's intent carries when it is dispatched on this draw.
    const fn record(self) -> PolicyRandomDelay {
        PolicyRandomDelay {
            due_at_unix_ms: self.due_at_unix_ms,
            planned_start_unix_ms: self.planned_start_unix_ms,
            delay_ms: self.random_delay_ms,
        }
    }
}

/// The evaluation's decision of `task_id` on the fixture instance.
fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id}: {evaluation:#?}"))
}

/// `task_id`'s `dispatch_random_delay_waiting` reason, `None` when its decision has none. A pair
/// that waits names itself, is deferred and is not dispatched, and its planned start is its
/// `due_at` plus its delay.
fn waiting(evaluation: &PolicyEvaluation, task_id: &str) -> Option<Waiting> {
    let decision = decision(evaluation, task_id);
    let reason = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(HostCode::DispatchRandomDelayWaiting))?;
    let outcome = &reason.outcome;
    assert_eq!(
        outcome.value("task_id"),
        Some(&json!(task_id)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS)),
        "{outcome:?}"
    );
    let time = |key: &str| {
        outcome
            .value(key)
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("{key} in {outcome:?}"))
    };
    let waiting = Waiting {
        due_at_unix_ms: time("due_at_unix_ms"),
        random_delay_ms: time("random_delay_ms"),
        planned_start_unix_ms: time("next_eligible_unix_ms"),
    };
    assert_eq!(
        waiting.planned_start_unix_ms,
        waiting.due_at_unix_ms + waiting.random_delay_ms,
        "{outcome:?}"
    );
    assert_eq!(decision.state, SchedulingDecisionState::Deferred);
    assert!(
        dispatched(evaluation, task_id).is_none(),
        "a waiting pair is not dispatched: {evaluation:#?}"
    );
    Some(waiting)
}

/// `task_id`'s fresh draw at `due_at_unix_ms`: it waits, and its delay lies within the bounds.
fn drawn(evaluation: &PolicyEvaluation, task_id: &str, due_at_unix_ms: u64) -> Waiting {
    let waiting = waiting(evaluation, task_id)
        .unwrap_or_else(|| panic!("{task_id} waits a random delay: {evaluation:#?}"));
    assert_eq!(waiting.due_at_unix_ms, due_at_unix_ms, "{task_id}'s draw");
    assert!(
        (MIN_MS..=MAX_MS).contains(&waiting.random_delay_ms),
        "{task_id}'s delay {} lies within [{MIN_MS}, {MAX_MS}]",
        waiting.random_delay_ms
    );
    waiting
}

/// The evaluation's dispatch of `task_id` with its reason chain, if it has one.
fn dispatched(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == POLICY_INSTANCE_ALIAS)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

// ---- The fixture.

fn open_host(
    root: &TempDir,
    instance: InstanceId,
    state: &Arc<FakeState>,
    clock: &Arc<ManualRuntimeClock>,
    manifest: RuntimeConfigManifest,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts(None), policy_resources()))
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::clone(state),
        )),
    )
    .expect("start the random delay host")
}

struct Fixture {
    root: TempDir,
    instance: InstanceId,
    state: Arc<FakeState>,
    manifest: RuntimeConfigManifest,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
    approved: bool,
}

impl Fixture {
    /// A host whose instance sets its own random delay (`enabled`, `min_ms`, `max_ms`), with the
    /// catalog of `tasks` active and its clock at `at_unix_ms`.
    fn start(tasks: &[SlotTask], enabled: bool, min_ms: u64, max_ms: u64, at_unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let instance = instance_id();
        let state = Arc::new(FakeState::default());
        let manifest = delay_manifest(instance, enabled, min_ms, max_ms);
        let clock = Arc::new(ManualRuntimeClock::new(at_unix_ms, 0));
        let host = open_host(&root, instance, &state, &clock, manifest.clone());
        host.activate_policy_catalog(&slot_sources(1, tasks, None))
            .expect("activate the catalog");
        Self {
            root,
            instance,
            state,
            manifest,
            clock,
            host,
            approved: false,
        }
    }

    /// The host closed and started again on the same state root, configuration and instance,
    /// its clock at `at_unix_ms`.
    fn restart(self, at_unix_ms: u64) -> Self {
        let Self {
            root,
            instance,
            state,
            manifest,
            host,
            approved,
            ..
        } = self;
        host.close().expect("close the host");
        let clock = Arc::new(ManualRuntimeClock::new(at_unix_ms, 0));
        let host = open_host(&root, instance, &state, &clock, manifest.clone());
        Self {
            root,
            instance,
            state,
            manifest,
            clock,
            host,
            approved,
        }
    }

    fn close(self) {
        self.host.close().expect("close the host");
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time, as its capacity
    /// monitor does while time passes.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One full evaluation at `unix_ms` on `facts`.
    fn evaluate(&self, unix_ms: u64, facts: &EvaluationFacts) -> PolicyEvaluation {
        self.set_time(unix_ms);
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    /// The catalog's approval, recorded once.
    fn approve(&mut self, intent: &DispatchIntent) {
        if !self.approved {
            record_policy_approval(&self.host, intent);
            self.approved = true;
        }
    }

    /// Admits `intent` as the scheduler does, completes its run and ends its lease.
    fn run(&mut self, intent: &DispatchIntent, chain: &DecisionReasonChain) {
        self.approve(intent);
        let admission = self
            .host
            .admit_policy_dispatch(intent, chain, &policy_context(&self.host, intent))
            .unwrap_or_else(|error| panic!("admit {}: {error}", intent.task_id));
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("{} is granted", intent.task_id);
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    /// The admission of `intent` while a keyless observation holds the instance's admission
    /// guard: its refusal.
    fn contended_admission(
        &self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
    ) -> RuntimeHostError {
        let guard = self
            .host
            .instance_admission_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the instance's admission guard");
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = thread::spawn(move || {
            let _held = guard.lock().expect("hold the admission guard");
            held_tx.send(()).expect("report the held guard");
            let _ = release_rx.recv();
        });
        held_rx.recv_timeout(GATE_WAIT).expect("the guard is held");
        let refused = self
            .host
            .admit_policy_dispatch(intent, chain, &policy_context(&self.host, intent))
            .expect_err("a contended admission is refused");
        release_tx.send(()).expect("release the guard");
        holder.join().expect("the guard's holder");
        refused
    }

    /// A client lease that holds the instance until it is released.
    fn hold(&self) -> (TestClient, LeaseToken) {
        let mut holder = TestClient::connect(&self.host);
        let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS);
        (holder, token)
    }

    fn release((mut holder, token): (TestClient, LeaseToken)) {
        let release = holder.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            holder.send(&release).state(),
            RuntimeReceiptState::Completed
        );
    }

    /// One `ConfigureSettings` of the instance's own `member`, as `actingctl config set` sends it.
    fn configure(&self, member: &str, value: Value) {
        let key = instance_key(
            &crate::failure_identity::identifier_text(&self.instance),
            member,
        );
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key,
                value: Some(value),
            }],
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// The `random_delay` member of every `event_type` record of `task_id`, in ledger order.
    fn delays(&self, event_type: EventType, task_id: &str) -> Vec<Option<PolicyRandomDelay>> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(event_type),
                ..EventQuery::default()
            })
            .expect("ledger records")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(
                    PolicyPayload::DispatchIntent(payload)
                    | PolicyPayload::DispatchAdmitted(payload)
                    | PolicyPayload::DispatchRejected(payload)
                    | PolicyPayload::DispatchCompleted(payload),
                ) if payload.task_id() == task_id => Some(payload.random_delay().copied()),
                _ => None,
            })
            .collect()
    }

    /// No admitted or completed record of `task_id` carries a random delay.
    fn assert_only_intents_carry_it(&self, task_id: &str) {
        for event_type in [
            EventType::PolicyDispatchAdmitted,
            EventType::PolicyDispatchCompleted,
        ] {
            assert!(
                self.delays(event_type, task_id).iter().all(Option::is_none),
                "{event_type:?} of {task_id} carries no random delay"
            );
        }
    }
}

// ---- Requirement 2 (off) and 4: an instance with the delay off is not gated and records nothing.
#[test]
fn d1_a_disabled_instance_dispatches_at_once_without_a_delay_member() {
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        false,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let evaluation = fixture.evaluate(NOON, &facts(None));
    assert_eq!(waiting(&evaluation, TASK_A), None, "nothing is drawn");
    let (intent, chain) =
        dispatched(&evaluation, TASK_A).expect("the due task is dispatched at once");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [None],
        "the intent of an instance with the delay off carries none"
    );
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchAdmitted, TASK_A),
        [None]
    );
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchCompleted, TASK_A),
        [None],
        "an instance with the delay off writes none"
    );
    fixture.close();
}

// ---- Requirements 2 and 4: a due pair draws `due_at` = the evaluation time and a delay within
// [min_ms, max_ms], waits with `dispatch_random_delay_waiting` and a wake at its planned start,
// still waits 1 ms before it, and is dispatched at it; its intent records the draw, its admitted
// record does not.
#[test]
fn d1_an_enabled_instance_waits_its_drawn_delay_and_records_it_on_the_intent() {
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(None));
    let draw = drawn(&first, TASK_A, NOON);
    assert_eq!(
        first.next_wake_unix_ms,
        Some(draw.planned_start_unix_ms),
        "the evaluation wakes at the planned start"
    );

    let before = fixture.evaluate(draw.planned_start_unix_ms - 1, &facts(None));
    assert_eq!(
        waiting(&before, TASK_A),
        Some(draw),
        "1 ms before its planned start the pair still waits its draw"
    );

    let at_start = fixture.evaluate(draw.planned_start_unix_ms, &facts(None));
    assert_eq!(waiting(&at_start, TASK_A), None);
    let (intent, chain) =
        dispatched(&at_start, TASK_A).expect("the pair is dispatched at its planned start");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(draw.record())],
        "the intent records the draw"
    );
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchAdmitted, TASK_A),
        [None]
    );
    fixture.assert_only_intents_carry_it(TASK_A);
    fixture.close();
}

// ---- Requirement 2 (Q6, Q7): every due pair of the instance draws its own delay in the same
// evaluation, and a dependency follow-up that becomes due after its dependency ran draws a fresh
// one; a pair the higher-ranked dispatch kept off the instance keeps its draw.
#[test]
fn d1_each_due_task_and_a_dependency_follow_up_draw_their_own_delay() {
    let tasks = [
        task(TASK_A, 300, daily(NOON_MINUTE)),
        task(TASK_B, 200, daily(NOON_MINUTE)),
        task(TASK_C, 400, dependency(TASK_A)),
    ];
    let mut fixture = Fixture::start(&tasks, true, MIN_MS, MAX_MS, NOON);
    let first = fixture.evaluate(NOON, &facts(None));
    let draw_a = drawn(&first, TASK_A, NOON);
    let draw_b = drawn(&first, TASK_B, NOON);
    assert_ne!(
        draw_a.random_delay_ms, draw_b.random_delay_ms,
        "each pair draws its own delay"
    );
    assert_eq!(
        waiting(&first, TASK_C),
        None,
        "the follow-up is not due before its dependency ran"
    );

    let both_ended = draw_a
        .planned_start_unix_ms
        .max(draw_b.planned_start_unix_ms);
    let second = fixture.evaluate(both_ended, &facts(None));
    let (intent, chain) =
        dispatched(&second, TASK_A).expect("the higher-ranked pair whose delay ended runs");
    fixture.run(&intent, &chain);

    let follow_up_at = both_ended + MINUTE_MS;
    let third = fixture.evaluate(follow_up_at, &facts(None));
    drawn(&third, TASK_C, follow_up_at);
    let (intent, chain) =
        dispatched(&third, TASK_B).expect("the other pair runs on its first draw");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(draw_a.record())]
    );
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_B),
        [Some(draw_b.record())]
    );
    fixture.close();
}

// ---- Requirement 2 (Q9): the delay is cut at the activity window's last millisecond and at the
// facts freshness of the decision; a window that ends right after the draw leaves a delay of 0,
// dispatched at once with `delay_ms` 0.
#[test]
fn d1_the_delay_is_cut_at_the_window_end_and_at_the_facts_freshness() {
    const LONG_MIN_MS: u64 = 30 * MINUTE_MS;

    // A draw of at least 30 min at 23:20 is cut to the window's last millisecond.
    let late = at(0, 23, 20);
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(23 * 60 + 20))],
        true,
        LONG_MIN_MS,
        MAX_MS,
        late,
    );
    let evaluation = fixture.evaluate(late, &facts(None));
    let cut = Waiting {
        due_at_unix_ms: late,
        random_delay_ms: WINDOW_LAST - late,
        planned_start_unix_ms: WINDOW_LAST,
    };
    assert_eq!(waiting(&evaluation, TASK_A), Some(cut));
    let at_end = fixture.evaluate(WINDOW_LAST, &facts(None));
    let (intent, chain) =
        dispatched(&at_end, TASK_A).expect("dispatched at the window's last millisecond");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(cut.record())]
    );
    fixture.close();

    // Facts fresh for 5 min cut a draw of at least 30 min to 5 min.
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, ready_trigger(Some(5 * MINUTE_MS)))],
        true,
        LONG_MIN_MS,
        MAX_MS,
        NOON,
    );
    let ready = facts(Some((true, NOON)));
    let fresh_until = NOON + 5 * MINUTE_MS;
    let evaluation = fixture.evaluate(NOON, &ready);
    let cut = Waiting {
        due_at_unix_ms: NOON,
        random_delay_ms: 5 * MINUTE_MS,
        planned_start_unix_ms: fresh_until,
    };
    assert_eq!(waiting(&evaluation, TASK_A), Some(cut));
    let at_end = fixture.evaluate(fresh_until, &ready);
    let (intent, chain) =
        dispatched(&at_end, TASK_A).expect("dispatched while its facts are still fresh");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(cut.record())]
    );
    fixture.close();

    // At the window's last millisecond nothing is left: delay 0, dispatched at once.
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(23 * 60 + 29))],
        true,
        MIN_MS,
        MAX_MS,
        WINDOW_LAST,
    );
    let evaluation = fixture.evaluate(WINDOW_LAST, &facts(None));
    assert_eq!(
        waiting(&evaluation, TASK_A),
        None,
        "nothing is left to wait"
    );
    let (intent, chain) = dispatched(&evaluation, TASK_A).expect("dispatched at once");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(PolicyRandomDelay {
            due_at_unix_ms: WINDOW_LAST,
            planned_start_unix_ms: WINDOW_LAST,
            delay_ms: 0,
        })]
    );
    fixture.close();
}

// ---- Requirement 3 (Q8): a pair held past its planned start keeps its draw and is dispatched at
// once, with the same values, when the instance is free; a pair that waits when the instance's
// `enabled` turns false through a configuration instruction is dispatched at the next evaluation
// without a member.
#[test]
fn d1_a_held_instance_keeps_the_draw_and_turning_the_delay_off_releases_it() {
    let tasks = [
        task(TASK_A, 300, daily(NOON_MINUTE)),
        task(TASK_B, 200, ready_trigger(None)),
    ];
    let mut fixture = Fixture::start(&tasks, true, MIN_MS, MAX_MS, NOON);
    let not_ready = facts(Some((false, NOON)));
    let first = fixture.evaluate(NOON, &not_ready);
    let draw_a = drawn(&first, TASK_A, NOON);

    let holder = fixture.hold();
    let held_at = draw_a.planned_start_unix_ms + MINUTE_MS;
    let held = fixture.evaluate(held_at, &not_ready);
    assert!(
        decision(&held, TASK_A)
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(HostCode::DispatchInstanceHeld)),
        "the held instance defers the pair first: {held:#?}"
    );
    assert!(dispatched(&held, TASK_A).is_none());
    Fixture::release(holder);

    let free_at = held_at + MINUTE_MS;
    let free = fixture.evaluate(free_at, &not_ready);
    assert_eq!(waiting(&free, TASK_A), None, "no second delay");
    let (intent, chain) =
        dispatched(&free, TASK_A).expect("dispatched at once once the instance is free");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(draw_a.record())],
        "the intent carries the draw made before the hold"
    );

    let ready_at = free_at + MINUTE_MS;
    let ready = facts(Some((true, ready_at)));
    let second = fixture.evaluate(ready_at, &ready);
    let draw_b = drawn(&second, TASK_B, ready_at);
    fixture.configure(RANDOM_DELAY_ENABLED, json!(false));
    let released_at = ready_at + 1_000;
    assert!(released_at < draw_b.planned_start_unix_ms);
    let released = fixture.evaluate(released_at, &ready);
    assert_eq!(waiting(&released, TASK_B), None);
    let (intent, chain) =
        dispatched(&released, TASK_B).expect("the waiting pair is released at once");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_B),
        [None],
        "a dispatch with the delay off carries no member"
    );
    fixture.close();
}

// ---- Requirement 3: an evaluation whose trigger is false ends the draw; the next time the pair
// is due it draws anew, with `due_at` that evaluation's time.
#[test]
fn d1_a_false_trigger_ends_the_draw_and_the_next_due_draws_anew() {
    let fixture = Fixture::start(
        &[task(TASK_A, 300, ready_trigger(None))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(Some((true, NOON))));
    let draw = drawn(&first, TASK_A, NOON);

    let false_at = NOON + 1_000;
    let not_due = fixture.evaluate(false_at, &facts(Some((false, false_at))));
    assert_eq!(waiting(&not_due, TASK_A), None);
    assert_eq!(
        decision(&not_due, TASK_A).state,
        SchedulingDecisionState::Blocked,
        "the trigger is false"
    );

    let due_again_at = NOON + 2_000;
    assert!(due_again_at < draw.planned_start_unix_ms);
    let again = fixture.evaluate(due_again_at, &facts(Some((true, due_again_at))));
    drawn(&again, TASK_A, due_again_at);
    fixture.close();
}

// ---- Requirement 3 (Q12): an admission refused by contention keeps the draw: the next
// evaluation dispatches the pair at once with the same `due_at`; the rejection carries no member.
#[test]
fn d1_a_rejected_admission_keeps_the_draw() {
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(None));
    let draw = drawn(&first, TASK_A, NOON);
    let at_start = fixture.evaluate(draw.planned_start_unix_ms, &facts(None));
    let (intent, chain) = dispatched(&at_start, TASK_A).expect("dispatched at its planned start");
    fixture.approve(&intent);
    let refused = fixture.contended_admission(&intent, &chain);
    assert_eq!(refused.code(), HostCode::DispatchInstanceContended.as_str());
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchRejected, TASK_A),
        [None],
        "a rejection carries no random delay"
    );

    let retry_at = draw.planned_start_unix_ms + MINUTE_MS;
    let retry = fixture.evaluate(retry_at, &facts(None));
    assert_eq!(waiting(&retry, TASK_A), None, "no second delay");
    let (intent, chain) = dispatched(&retry, TASK_A).expect("dispatched at once on its draw");
    fixture.run(&intent, &chain);
    assert_eq!(
        fixture.delays(EventType::PolicyDispatchIntent, TASK_A),
        [Some(draw.record()), Some(draw.record())],
        "both intents carry the one draw"
    );
    fixture.close();
}

// ---- Requirement 3 (register X14): the draws live in memory only; after a restart on the same
// root the first evaluation draws anew with `due_at` its own time.
#[test]
fn d1_a_restart_draws_anew() {
    let fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(None));
    let draw = drawn(&first, TASK_A, NOON);

    let restart_at = NOON + 1_000;
    assert!(restart_at < draw.planned_start_unix_ms);
    let fixture = fixture.restart(restart_at);
    let after = fixture.evaluate(restart_at, &facts(None));
    drawn(&after, TASK_A, restart_at);
    fixture.close();
}

// ---- Requirement 5: a direct run on an instance whose pair waits its delay starts at once and
// completes; the waiting pair takes nothing from the instance and is not dispatched by it.
#[test]
fn d1_a_direct_run_on_a_waiting_instance_starts_at_once() {
    let fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(None));
    drawn(&first, TASK_A, NOON);

    let package = super::gate_369s5::direct_run_package(&fixture.root, &fixture.state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.as_str()).expect("task request"),
        ),
    );
    let receipt = fixture
        .host
        .process_request_for_test(&request, ConnectionId::new(4051).expect("connection"))
        .expect("the direct run's receipt");
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
    assert!(
        fixture
            .delays(EventType::PolicyDispatchIntent, TASK_A)
            .is_empty(),
        "the direct run dispatches no policy task"
    );
    fixture.close();
}

// ---- Requirement 6: `dispatch_random_delay_waiting` is a waiting gate for the clock-slot
// records: a slot that waits for its delay inside its window is neither refused nor deferred.
#[test]
fn d1_a_waiting_clock_slot_is_not_refused() {
    let mut fixture = Fixture::start(
        &[task(TASK_A, 300, daily(NOON_MINUTE))],
        true,
        MIN_MS,
        MAX_MS,
        NOON,
    );
    let first = fixture.evaluate(NOON, &facts(None));
    let draw = drawn(&first, TASK_A, NOON);
    fixture.evaluate(draw.planned_start_unix_ms - 1, &facts(None));
    let rows = slot_rows(&fixture.host);
    assert!(
        rows.is_empty(),
        "a slot that waits for its delay inside its window leaves no record: {rows:#?}"
    );

    let at_start = fixture.evaluate(draw.planned_start_unix_ms, &facts(None));
    let (intent, chain) = dispatched(&at_start, TASK_A).expect("dispatched at its planned start");
    fixture.run(&intent, &chain);
    let rows = slot_rows(&fixture.host);
    assert!(
        rows.iter()
            .all(|row| row.kind != "refused" && row.kind != "deferred"),
        "the slot is served, never refused or deferred: {rows:#?}"
    );
    fixture.close();
}
