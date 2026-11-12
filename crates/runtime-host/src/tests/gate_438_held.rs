// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #438 HELD-1 (register §1.322 rows HELD-R1 to HELD-R9, "The code slices", §1.327;
//! `contracts/scheduling/README.md`, "Held records"): the slice's named tests `held_t1` to
//! `held_t10` (`held_t9`'s diagnose half is lab-ops' `gate_438_held`).
//!
//! The logic tests drive the not-run table and the held memory the policy host keeps, one
//! evaluated cycle at a time on a manual clock: each cycle records an evaluation's decisions in
//! the table, the memory reads it, and the records it returns are then in a fake planning-signal
//! projection, as the cycle's recorder appends them. The host tests run the fixture instance on a
//! manual Runtime clock through the policy cycle, the start, the ledger and `actingd suspended`.
//! Every task, key and alias is a neutral name; every comparison is within one host but `held_t6`'s,
//! which compares decisions, reason codes and records, never an identity.

use super::clock_slot_report::{ledger_events, record_sequence, report};
use super::gate_372k2::{SlotTask, Slots, at, slot_sources};
use super::*;
use crate::codes::{HeldEnd, HeldState, HostCode, SchedulingRuleHostCode, TaskHeldCode};
use crate::policy_host::held::{
    EndedHold, HeldMemory, HeldRow, ended_signal, ended_signal_id, held_signal, held_signal_id,
};
use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::outcome::{CauseRelation, Code, Outcome};
use actingcommand_contract::{PolicyPlanningSignalPayload, ScheduleDispatchState};
use actingcommand_policy::{
    DecisionReason, EligibilityState, FACT_REFRESH_TASK_ID, PolicyCode, PolicyEvaluation,
    SchedulingDecisionState, TaskDecision,
};
use actingcommand_runtime_config::TaskHeldConfig;
use serde_json::{Value, json};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// The start of every logic test's clock.
const T0: u64 = 1_700_000_000_000;
const ALIAS: &str = "neutral-a";
const OTHER: &str = "neutral-b";
const HEAD: &str = "task.neutral.head";
const FIRST: &str = "task.neutral.first";
const SECOND: &str = "task.neutral.second";
/// The defaults of `scheduling.task_held.*`: on, 30 minutes.
const DEFAULT: TaskHeldConfig = TaskHeldConfig {
    enabled: true,
    after_ms: 30 * MINUTE_MS,
};
const PREFIX: &str = "signal:task_held:";

// ---- The logic tests' harness.

/// A `why` outcome as the evaluation gives it, read back as a received outcome: `code` with
/// `values`.
fn why(code: impl Into<Code>, values: Value) -> Outcome {
    let code: Code = code.into();
    serde_json::from_value(json!({
        "code": code.as_str(),
        "category": code.category().as_str(),
        "values": values,
    }))
    .expect("a why outcome")
}

/// An unselected decision of `task_id` on `alias` with `eligibility`, its last reason `reason`.
fn row_on(
    alias: &str,
    task_id: &str,
    eligibility: EligibilityState,
    reason: Outcome,
) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some(alias.to_owned()),
        eligibility,
        state: SchedulingDecisionState::Deferred,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![DecisionReason {
            outcome: reason,
            detail: String::new(),
        }],
        next_wake_unix_ms: None,
    }
}

fn row(task_id: &str, eligibility: EligibilityState, reason: Outcome) -> TaskDecision {
    row_on(ALIAS, task_id, eligibility, reason)
}

/// The selected decision of `task_id` on the instance.
fn selected(task_id: &str) -> TaskDecision {
    TaskDecision {
        task_id: task_id.to_owned(),
        instance_id: Some(ALIAS.to_owned()),
        eligibility: EligibilityState::True,
        state: SchedulingDecisionState::Selected,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: Vec::new(),
        next_wake_unix_ms: None,
    }
}

fn unavailable() -> Outcome {
    why(PolicyCode::InstanceUnavailable, json!({}))
}

fn held_instance() -> Outcome {
    why(
        HostCode::DispatchInstanceHeld,
        json!({"holder_kind": "client"}),
    )
}

fn trigger_unknown() -> Outcome {
    why(PolicyCode::TriggerUnknown, json!({}))
}

fn trigger_false() -> Outcome {
    why(PolicyCode::TriggerFalse, json!({}))
}

/// A wait for one fact's refresh, `code` one of the three, of `fact_key`.
fn refresh_wait(code: PolicyCode, fact_key: &str) -> Outcome {
    why(
        code,
        json!({
            "instance_id": ALIAS,
            "fact_key": fact_key,
            "unknown_reason": "missing",
            "package_id": "neutral.reader",
        }),
    )
}

/// One run of the Runtime as the held records see it: the not-run table, the held memory, the
/// planning-signal projection the records were appended to, each instance's setting and the pairs
/// in flight.
struct Harness {
    table: ScheduleExplanationTable,
    memory: HeldMemory,
    projection: BTreeMap<String, PolicyPlanningSignalEventData>,
    settings: BTreeMap<String, TaskHeldConfig>,
    in_flight: BTreeSet<(String, String)>,
}

impl Harness {
    /// One instance, `ALIAS`, with the defaults.
    fn new() -> Self {
        Self {
            table: ScheduleExplanationTable::default(),
            memory: HeldMemory::default(),
            projection: BTreeMap::new(),
            settings: BTreeMap::from([(ALIAS.to_owned(), DEFAULT)]),
            in_flight: BTreeSet::new(),
        }
    }

    /// The evaluated cycle at `at` of `decisions`: the table replaced, then the records the held
    /// memory returns, which are then in the projection.
    fn cycle(
        &mut self,
        at: u64,
        decisions: Vec<TaskDecision>,
    ) -> Vec<PolicyPlanningSignalEventData> {
        let evaluation = PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        };
        self.table
            .record(&evaluation, "sha256:neutral-held", at, &|_: &str,
                                                              _: &str|
             -> Option<(
                u64,
                String,
            )> {
                None
            });
        let in_flight = self
            .in_flight
            .iter()
            .map(|(task_id, alias)| (task_id.as_str(), alias.as_str()))
            .collect::<BTreeSet<_>>();
        let signals = self
            .memory
            .observe(
                &self.table,
                &evaluation,
                &in_flight,
                &|alias: &str| self.settings.get(alias).copied(),
                &|signal_id: &str| -> RuntimeHostResult<bool> {
                    Ok(self.projection.contains_key(signal_id))
                },
                at,
            )
            .expect("the held records of the cycle");
        for signal in &signals {
            self.projection
                .insert(signal.signal_id.clone(), signal.clone());
        }
        signals
    }

    /// The row of `task_id` on the instance as `ac_schedule` answers it: its `since_unix_ms`.
    fn since(&self, task_id: &str) -> u64 {
        let answer = self
            .table
            .answer(Some(ALIAS), true, |_| ScheduleDispatchState::Pending);
        answer.instances[0]
            .candidates
            .iter()
            .find(|row| row.task_id == task_id)
            .unwrap_or_else(|| panic!("a row of {task_id}: {answer:?}"))
            .since_unix_ms
    }
}

/// A record's value `key` as JSON.
fn value<'a>(signal: &'a PolicyPlanningSignalEventData, key: &str) -> Option<&'a Value> {
    signal.outcome.value(key)
}

/// Asserts `signal` is the held record of `task_id` on `alias` that started at `since`, written
/// at `now` under `after_ms`, of `gate_code` and `state`, linked to `cause`.
#[allow(clippy::too_many_arguments)]
fn assert_held(
    signal: &PolicyPlanningSignalEventData,
    alias: &str,
    task_id: &str,
    gate_code: Code,
    state: HeldState,
    since: u64,
    now: u64,
    after_ms: u64,
    cause: &Outcome,
) {
    assert_eq!(signal.signal_id, held_signal_id(task_id, alias, since));
    assert_eq!(
        signal.kind,
        PolicyPlanningSignalKind::GoalMissed,
        "{signal:?}"
    );
    assert_eq!(signal.instance_id, alias, "{signal:?}");
    assert_eq!(signal.task_id.as_deref(), Some(task_id), "{signal:?}");
    assert_eq!(signal.observed_at_unix_ms, now, "{signal:?}");
    assert!(signal.outcome.is(TaskHeldCode::Held), "{signal:?}");
    assert_eq!(
        signal.outcome.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert_eq!(value(signal, "task_id"), Some(&json!(task_id)));
    assert_eq!(value(signal, "alias"), Some(&json!(alias)));
    assert_eq!(value(signal, "gate_code"), Some(&json!(gate_code.as_str())));
    assert_eq!(value(signal, "held_state"), Some(&json!(state.as_str())));
    assert_eq!(value(signal, "held_since_unix_ms"), Some(&json!(since)));
    assert_eq!(value(signal, "held_for_ms"), Some(&json!(now - since)));
    assert_eq!(value(signal, "held_after_ms"), Some(&json!(after_ms)));
    let causes = signal.outcome.causes();
    assert_eq!(causes.len(), 1, "one link: {signal:?}");
    assert_eq!(causes[0].relation(), &CauseRelation::CausedBy);
    assert_eq!(causes[0].outcome(), cause, "the row's why, whole");
}

/// Asserts `signal` is the end `end` of the hold of `task_id` on `alias` that started at `since`
/// and was found ended at `now` (`held_for_ms` `now - since`).
fn assert_ended(
    signal: &PolicyPlanningSignalEventData,
    alias: &str,
    task_id: &str,
    gate_code: Code,
    since: u64,
    end: HeldEnd,
    now: u64,
) {
    assert_eq!(signal.signal_id, ended_signal_id(task_id, alias, since));
    assert_eq!(
        signal.kind,
        PolicyPlanningSignalKind::GoalMissed,
        "{signal:?}"
    );
    assert_eq!(signal.instance_id, alias, "{signal:?}");
    assert_eq!(signal.task_id.as_deref(), Some(task_id), "{signal:?}");
    assert_eq!(signal.observed_at_unix_ms, now, "{signal:?}");
    assert!(signal.outcome.is(TaskHeldCode::Ended), "{signal:?}");
    assert_eq!(
        signal.outcome.category(),
        actingcommand_contract::outcome::Category::Info
    );
    assert_eq!(value(signal, "gate_code"), Some(&json!(gate_code.as_str())));
    assert_eq!(value(signal, "held_since_unix_ms"), Some(&json!(since)));
    assert_eq!(value(signal, "held_end"), Some(&json!(end.as_str())));
    assert_eq!(value(signal, "held_for_ms"), Some(&json!(now - since)));
    assert!(signal.outcome.causes().is_empty(), "{signal:?}");
}

/// The one signal of `signals` about `task_id`.
fn of<'a>(
    signals: &'a [PolicyPlanningSignalEventData],
    task_id: &str,
) -> &'a PolicyPlanningSignalEventData {
    let found = signals
        .iter()
        .filter(|signal| signal.task_id.as_deref() == Some(task_id))
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one signal of {task_id}: {signals:#?}");
    found[0]
}

// ---- The logic tests.

/// 11-11's shape in neutral names: a head held by the no-progress guard and two followers whose
/// triggers are unknown. Nothing before the threshold; at the first evaluation past it one
/// `task_held` each, the head `due` with its guard's reason as its link, the followers
/// `undecidable`; nothing more at later evaluations; when the head is selected its end `selected`,
/// and the followers' ends `why_changed` when their rows change.
#[test]
fn held_t1_a_chain_head_held_by_its_guard_and_its_unknown_followers_are_reported_once_after_the_threshold_and_end_when_the_head_runs()
 {
    let guard = why(
        PolicyCode::PolicyNoProgress,
        json!({
            "basis_readings": [{
                "fact_key": "resource.neutral.amount",
                "reading_value": "12",
                "observed_at_unix_ms": T0 - 1_000_000,
            }],
            "served_by_dispatch_unix_ms": T0 - 2_000_000,
        }),
    );
    let waiting = || {
        vec![
            row(HEAD, EligibilityState::True, guard.clone()),
            row(FIRST, EligibilityState::Unknown, trigger_unknown()),
            row(SECOND, EligibilityState::Unknown, trigger_unknown()),
        ]
    };
    let mut held = Harness::new();
    for minutes in [0, 10, 29] {
        assert!(
            held.cycle(T0 + minutes * MINUTE_MS, waiting()).is_empty(),
            "nothing before the threshold ({minutes} min)"
        );
    }
    let past = T0 + 30 * MINUTE_MS;
    let signals = held.cycle(past, waiting());
    assert_eq!(signals.len(), 3, "one held record each: {signals:#?}");
    assert_held(
        of(&signals, HEAD),
        ALIAS,
        HEAD,
        PolicyCode::PolicyNoProgress.code(),
        HeldState::Due,
        T0,
        past,
        DEFAULT.after_ms,
        &guard,
    );
    for follower in [FIRST, SECOND] {
        assert_held(
            of(&signals, follower),
            ALIAS,
            follower,
            PolicyCode::TriggerUnknown.code(),
            HeldState::Undecidable,
            T0,
            past,
            DEFAULT.after_ms,
            &trigger_unknown(),
        );
    }
    for minutes in [31, 40, 45] {
        assert!(
            held.cycle(T0 + minutes * MINUTE_MS, waiting()).is_empty(),
            "one hold is reported once ({minutes} min)"
        );
    }

    // The head's reading changed and it is selected: its end; the followers still wait.
    let ran = T0 + 50 * MINUTE_MS;
    let signals = held.cycle(
        ran,
        vec![
            selected(HEAD),
            row(FIRST, EligibilityState::Unknown, trigger_unknown()),
            row(SECOND, EligibilityState::Unknown, trigger_unknown()),
        ],
    );
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_ended(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::PolicyNoProgress.code(),
        T0,
        HeldEnd::Selected,
        ran,
    );

    // The followers' triggers are decided: their rows change, and their holds end.
    let decided = T0 + 60 * MINUTE_MS;
    let signals = held.cycle(
        decided,
        vec![
            row(
                HEAD,
                EligibilityState::True,
                why(PolicyCode::TaskCooldownActive, json!({})),
            ),
            row(FIRST, EligibilityState::False, trigger_false()),
            row(SECOND, EligibilityState::False, trigger_false()),
        ],
    );
    assert_eq!(signals.len(), 2, "{signals:#?}");
    for follower in [FIRST, SECOND] {
        assert_ended(
            of(&signals, follower),
            ALIAS,
            follower,
            PolicyCode::TriggerUnknown.code(),
            T0,
            HeldEnd::WhyChanged,
            decided,
        );
    }
    assert!(
        held.cycle(
            T0 + 2 * HOUR_MS,
            vec![
                row(
                    HEAD,
                    EligibilityState::True,
                    why(PolicyCode::TaskCooldownActive, json!({})),
                ),
                row(FIRST, EligibilityState::False, trigger_false()),
                row(SECOND, EligibilityState::False, trigger_false()),
            ],
        )
        .is_empty(),
        "nothing more"
    );
}

/// What a person, the configuration or the values said, what waits for its own time and what is
/// not due is never reported, each held past the threshold: a pause, a cooldown, a false trigger,
/// a blackout, an exhausted budget, a benefit with no price yet and the lines that hold a pool. A
/// row the held set names, on the same instance at the same times, is reported.
#[test]
fn held_t2_a_pair_said_no_waiting_for_its_own_time_or_not_due_is_never_reported() {
    let never = [
        (
            "a pause",
            EligibilityState::True,
            why(
                HostCode::DispatchPausedInstance,
                json!({"instance_id": ALIAS}),
            ),
        ),
        (
            "a cooldown",
            EligibilityState::True,
            why(PolicyCode::TaskCooldownActive, json!({})),
        ),
        ("a false trigger", EligibilityState::False, trigger_false()),
        (
            "a blackout",
            EligibilityState::True,
            why(
                SchedulingRuleHostCode::BlackoutActive,
                json!({"instance_id": ALIAS, "blackout_id": "neutral-quiet"}),
            ),
        ),
        (
            "an exhausted budget",
            EligibilityState::True,
            why(
                HostCode::PolicyBudgetExhausted,
                json!({"budget_dimension": "task_daily"}),
            ),
        ),
        (
            "a benefit with no effects",
            EligibilityState::True,
            why(
                PolicyCode::BenefitUnknown,
                json!({"benefit_gap": "no_effects"}),
            ),
        ),
        (
            "a benefit whose entry is pending",
            EligibilityState::True,
            why(
                PolicyCode::BenefitUnknown,
                json!({"benefit_gap": "entry_pending"}),
            ),
        ),
        (
            "a benefit whose valuation is pending",
            EligibilityState::True,
            why(
                PolicyCode::BenefitUnknown,
                json!({"benefit_gap": "valuation_pending"}),
            ),
        ),
        (
            "a benefit with no valuation",
            EligibilityState::True,
            why(
                PolicyCode::BenefitUnknown,
                json!({"benefit_gap": "valuation_missing"}),
            ),
        ),
        (
            "a hard line",
            EligibilityState::True,
            why(
                PolicyCode::PoolLineHeld,
                json!({"pool_line_hold": "would_cross_hard_line"}),
            ),
        ),
        (
            "a capped candidate a spent budget stops",
            EligibilityState::True,
            why(
                PolicyCode::PoolLineHeld,
                json!({
                    "pool_line_hold": "capped_not_dispatchable",
                    "gate_code": "policy_budget_exhausted",
                }),
            ),
        ),
    ];
    let control = "task.neutral.control";
    let decisions = || {
        let mut decisions = never
            .iter()
            .enumerate()
            .map(|(index, (_, eligibility, reason))| {
                row(
                    &format!("task.neutral.case{index:02}"),
                    *eligibility,
                    reason.clone(),
                )
            })
            .collect::<Vec<_>>();
        decisions.push(row(control, EligibilityState::True, unavailable()));
        decisions
    };
    let mut held = Harness::new();
    assert!(held.cycle(T0, decisions()).is_empty());
    for minutes in [30, 60, 120] {
        let now = T0 + minutes * MINUTE_MS;
        let signals = held.cycle(now, decisions());
        for (index, (what, _, _)) in never.iter().enumerate() {
            let task_id = format!("task.neutral.case{index:02}");
            assert!(
                signals
                    .iter()
                    .all(|signal| signal.task_id.as_deref() != Some(task_id.as_str())),
                "{what} is never reported ({minutes} min): {signals:#?}"
            );
        }
        if minutes == 30 {
            assert_eq!(signals.len(), 1, "{signals:#?}");
            assert_held(
                &signals[0],
                ALIAS,
                control,
                PolicyCode::InstanceUnavailable.code(),
                HeldState::Due,
                T0,
                now,
                DEFAULT.after_ms,
                &unavailable(),
            );
        } else {
            assert!(signals.is_empty(), "{signals:#?}");
        }
    }
}

/// Another `why` code ends a reported hold (`why_changed`) and starts a new count, which is
/// reported once it lasts the threshold again (a move between the three fact refresh waits of one
/// key excepted: `held_t10`).
#[test]
fn held_t4_another_why_code_ends_a_reported_hold_and_starts_a_new_count() {
    let mut held = Harness::new();
    held.cycle(T0, vec![row(HEAD, EligibilityState::True, unavailable())]);
    let reported = T0 + 30 * MINUTE_MS;
    let signals = held.cycle(
        reported,
        vec![row(HEAD, EligibilityState::True, unavailable())],
    );
    assert_eq!(signals.len(), 1, "{signals:#?}");

    let moved = T0 + 35 * MINUTE_MS;
    let signals = held.cycle(
        moved,
        vec![row(HEAD, EligibilityState::True, held_instance())],
    );
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_ended(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::InstanceUnavailable.code(),
        T0,
        HeldEnd::WhyChanged,
        moved,
    );
    assert_eq!(held.since(HEAD), moved, "another code starts another run");
    assert!(
        held.cycle(
            moved + 29 * MINUTE_MS,
            vec![row(HEAD, EligibilityState::True, held_instance())]
        )
        .is_empty(),
        "the new count has not lasted the threshold"
    );
    let again = moved + 30 * MINUTE_MS;
    let signals = held.cycle(
        again,
        vec![row(HEAD, EligibilityState::True, held_instance())],
    );
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_held(
        &signals[0],
        ALIAS,
        HEAD,
        HostCode::DispatchInstanceHeld.code(),
        HeldState::Due,
        moved,
        again,
        DEFAULT.after_ms,
        &held_instance(),
    );
    assert_ne!(
        signals[0].signal_id,
        held_signal_id(HEAD, ALIAS, T0),
        "a new hold, a new record"
    );
}

/// The switch and the threshold, per instance. Off: no new held record, and the end of a hold
/// already reported is still written. A lower threshold reports at the next evaluation every run
/// already past it; a higher one withdraws nothing, and the hold's end is still written.
#[test]
fn held_t5_the_switch_and_the_threshold_per_instance() {
    let off = TaskHeldConfig {
        enabled: false,
        after_ms: 30 * MINUTE_MS,
    };
    let mut held = Harness::new();
    held.settings.insert(OTHER.to_owned(), off);
    let both = |a: Outcome, b: Outcome| {
        vec![
            row_on(ALIAS, HEAD, EligibilityState::True, a),
            row_on(OTHER, HEAD, EligibilityState::True, b),
        ]
    };
    assert!(
        held.cycle(T0, both(unavailable(), unavailable()))
            .is_empty()
    );
    let reported = T0 + 30 * MINUTE_MS;
    let signals = held.cycle(reported, both(unavailable(), unavailable()));
    assert_eq!(
        signals.len(),
        1,
        "only the instance whose record is on: {signals:#?}"
    );
    assert_eq!(signals[0].instance_id, ALIAS);

    // Off for the first instance too: nothing new, and its reported hold's end is still written.
    held.settings.insert(ALIAS.to_owned(), off);
    assert!(
        held.cycle(T0 + 35 * MINUTE_MS, both(unavailable(), unavailable()))
            .is_empty()
    );
    let ended = T0 + 40 * MINUTE_MS;
    let signals = held.cycle(ended, both(trigger_false(), unavailable()));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_ended(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::InstanceUnavailable.code(),
        T0,
        HeldEnd::WhyChanged,
        ended,
    );

    // The second instance on, with a threshold above its run: nothing; then a lower one: its
    // run, already past it, is reported at the next evaluation.
    held.settings.insert(
        OTHER.to_owned(),
        TaskHeldConfig {
            enabled: true,
            after_ms: HOUR_MS,
        },
    );
    assert!(
        held.cycle(T0 + 45 * MINUTE_MS, both(trigger_false(), unavailable()))
            .is_empty()
    );
    let lower = TaskHeldConfig {
        enabled: true,
        after_ms: 40 * MINUTE_MS,
    };
    held.settings.insert(OTHER.to_owned(), lower);
    let lowered = T0 + 46 * MINUTE_MS;
    let signals = held.cycle(lowered, both(trigger_false(), unavailable()));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_held(
        &signals[0],
        OTHER,
        HEAD,
        PolicyCode::InstanceUnavailable.code(),
        HeldState::Due,
        T0,
        lowered,
        lower.after_ms,
        &unavailable(),
    );

    // A higher threshold withdraws nothing written, and the hold's end is still written.
    held.settings.insert(
        OTHER.to_owned(),
        TaskHeldConfig {
            enabled: true,
            after_ms: 2 * HOUR_MS,
        },
    );
    assert!(
        held.cycle(T0 + 50 * MINUTE_MS, both(trigger_false(), unavailable()))
            .is_empty()
    );
    let gone = T0 + 55 * MINUTE_MS;
    let signals = held.cycle(gone, both(trigger_false(), trigger_false()));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_ended(
        &signals[0],
        OTHER,
        HEAD,
        PolicyCode::InstanceUnavailable.code(),
        T0,
        HeldEnd::WhyChanged,
        gone,
    );
}

/// A pair with a dispatch in flight and the fact refresh read are never reported, whatever holds
/// them and for however long; a pair the held set names beside them is.
#[test]
fn held_t7_a_pair_in_flight_and_the_fact_refresh_read_are_never_reported() {
    let mut held = Harness::new();
    held.in_flight.insert((FIRST.to_owned(), ALIAS.to_owned()));
    let decisions = || {
        vec![
            row(HEAD, EligibilityState::True, unavailable()),
            row(FIRST, EligibilityState::True, held_instance()),
            row(FACT_REFRESH_TASK_ID, EligibilityState::True, unavailable()),
        ]
    };
    assert!(held.cycle(T0, decisions()).is_empty());
    let past = T0 + 30 * MINUTE_MS;
    let signals = held.cycle(past, decisions());
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_eq!(signals[0].task_id.as_deref(), Some(HEAD));
    for minutes in [60, 240] {
        assert!(
            held.cycle(T0 + minutes * MINUTE_MS, decisions()).is_empty(),
            "neither the pair in flight nor the read ({minutes} min)"
        );
    }
}

/// A row that moves between `fact_refresh_pending` and `fact_refresh_unresolved` of one key, then
/// to `fact_refresh_exhausted`, keeps its `since_unix_ms` (and `ac_schedule` shows it), is reported
/// once past the threshold, and its hold does not end at the moves; another key starts a new
/// count.
#[test]
fn held_t10_the_fact_refresh_waits_of_one_key_keep_one_clock() {
    const KEY: &str = "neutral.key.one";
    const OTHER_KEY: &str = "neutral.key.two";
    let wait = |code: PolicyCode, key: &str| {
        vec![row(
            HEAD,
            EligibilityState::Unknown,
            refresh_wait(code, key),
        )]
    };
    let mut held = Harness::new();
    for (minutes, code) in [
        (0, PolicyCode::FactRefreshPending),
        (10, PolicyCode::FactRefreshUnresolved),
        (20, PolicyCode::FactRefreshPending),
        (25, PolicyCode::FactRefreshUnresolved),
    ] {
        assert!(
            held.cycle(T0 + minutes * MINUTE_MS, wait(code, KEY))
                .is_empty()
        );
        assert_eq!(held.since(HEAD), T0, "one clock at {minutes} min");
    }
    let reported = T0 + 30 * MINUTE_MS;
    let exhausted = refresh_wait(PolicyCode::FactRefreshExhausted, KEY);
    let signals = held.cycle(reported, wait(PolicyCode::FactRefreshExhausted, KEY));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_held(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::FactRefreshExhausted.code(),
        HeldState::Undecidable,
        T0,
        reported,
        DEFAULT.after_ms,
        &exhausted,
    );
    for (minutes, code) in [
        (35, PolicyCode::FactRefreshPending),
        (36, PolicyCode::FactRefreshUnresolved),
    ] {
        assert!(
            held.cycle(T0 + minutes * MINUTE_MS, wait(code, KEY))
                .is_empty(),
            "the moves end nothing ({minutes} min)"
        );
        assert_eq!(held.since(HEAD), T0);
    }

    // Another key: another run.
    let other = T0 + 40 * MINUTE_MS;
    let signals = held.cycle(other, wait(PolicyCode::FactRefreshPending, OTHER_KEY));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_ended(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::FactRefreshExhausted.code(),
        T0,
        HeldEnd::WhyChanged,
        other,
    );
    assert_eq!(held.since(HEAD), other, "another key starts another run");
    let again = other + 30 * MINUTE_MS;
    let signals = held.cycle(again, wait(PolicyCode::FactRefreshUnresolved, OTHER_KEY));
    assert_eq!(signals.len(), 1, "{signals:#?}");
    assert_held(
        &signals[0],
        ALIAS,
        HEAD,
        PolicyCode::FactRefreshUnresolved.code(),
        HeldState::Undecidable,
        other,
        again,
        DEFAULT.after_ms,
        &refresh_wait(PolicyCode::FactRefreshUnresolved, OTHER_KEY),
    );
}

// ---- The host tests' fixture.

/// An interval trigger, due at every evaluation.
fn always() -> Value {
    json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1, "anchor_ms": 0}
    })
}

/// A UTC `daily` clock trigger at `minute` of the day.
fn daily_at(minute: u16) -> Value {
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

const HOST_HEAD: &str = "fixture.held.head";
const HOST_LATE: &str = "fixture.held.late";

/// Two tasks on the fixture instance: the head, due at every evaluation, and a daily task at
/// 08:20 UTC. The daily task never ran, so its first run is owed and it too is due from the first
/// evaluation (`gate_372k2`, `gate_clock_slot_first_run_is_owed_and_a_spent_slot_is_refused_once`).
fn held_sources() -> CatalogSources {
    slot_sources(
        1,
        &[
            SlotTask {
                id: HOST_HEAD,
                priority: 200,
                trigger: always(),
                daily_limit: 24,
            },
            SlotTask {
                id: HOST_LATE,
                priority: 100,
                trigger: daily_at(8 * 60 + 20),
                daily_limit: 24,
            },
        ],
        None,
    )
}

/// The `task_held` and `task_held_ended` records of the host's ledger, in ledger order: the
/// signal id, the task, the outcome and the observation time.
fn held_records(host: &RuntimeHost) -> Vec<(String, Option<String>, Outcome, u64)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    })
    .expect("planning signals")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
            if payload.kind() == PolicyPlanningSignalKind::GoalMissed
                && payload.signal_id().starts_with(PREFIX) =>
        {
            Some((
                payload.signal_id().to_owned(),
                payload.task_id().map(str::to_owned),
                payload.outcome().clone(),
                payload.observed_at_unix_ms(),
            ))
        }
        _ => None,
    })
    .collect()
}

/// A configuration whose only setting is `instance.<instance>.scheduling.task_held.enabled`.
fn task_held_manifest(
    instance: &InstanceId,
    enabled: bool,
) -> actingcommand_contract::RuntimeConfigManifest {
    let mut parameters = vec![actingcommand_contract::ConfigParameter {
        key: actingcommand_runtime_config::instance_key(
            &crate::failure_identity::identifier_text(instance),
            actingcommand_runtime_config::scheduling::TASK_HELD_ENABLED,
        ),
        value: actingcommand_contract::FactScalar::Boolean(enabled),
        source: actingcommand_contract::ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("the held-record key is in the key table");
    actingcommand_contract::RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// One host with the held record on or off on the fixture instance, on its manual clock, its
/// catalog active.
struct HeldHost {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    _root: TempDir,
}

impl HeldHost {
    fn start(enabled: bool, unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let mut facts = policy_facts();
        facts.outcomes.clear();
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_config_manifest(task_held_manifest(&registered, enabled)),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("start the held host");
        host.activate_policy_catalog(&held_sources())
            .expect("activate the held catalog");
        Self {
            host,
            clock,
            _root: root,
        }
    }

    /// The evaluated cycle at `unix_ms`, the capacity sampled at that time first.
    fn cycle(&self, unix_ms: u64) -> PolicyCycle {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
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

    /// A client lease that holds the instance until it is released.
    fn hold(&self) -> (TestClient, LeaseToken) {
        let mut holder = TestClient::connect(&self.host);
        let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS);
        (holder, token)
    }
}

fn release((mut holder, token): (TestClient, LeaseToken)) {
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
}

/// What a cycle decided, by value: each decision's task, instance, state, eligibility and reason
/// codes; each new intent's task and instance; the detection and clock-slot records.
type Decided = (
    Vec<(
        String,
        Option<String>,
        SchedulingDecisionState,
        EligibilityState,
        Vec<String>,
    )>,
    Vec<(String, String)>,
    Vec<(String, Outcome)>,
);

fn decided(cycle: &PolicyCycle) -> Decided {
    let decisions = cycle
        .evaluation
        .as_ref()
        .expect("an evaluated cycle")
        .decisions
        .iter()
        .map(|decision| {
            (
                decision.task_id.clone(),
                decision.instance_id.clone(),
                decision.state,
                decision.eligibility,
                decision
                    .reasons
                    .iter()
                    .map(|reason| reason.outcome.code().as_str().to_owned())
                    .collect(),
            )
        })
        .collect();
    let intents = cycle
        .pending_dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.clone(), intent.instance_id.clone()))
        .collect();
    let records = cycle
        .detection_planning_signals
        .iter()
        .chain(&cycle.clock_slot_signals)
        .map(|signal| (signal.signal_id.clone(), signal.outcome.clone()))
        .collect();
    (decisions, intents, records)
}

// ---- The host tests.

/// Through the host: a hold is reported once, also over later evaluations, as the projection
/// holds its id; the start writes `restarted` for each hold the run before left open, with no
/// `held_for_ms`, and nothing for a hold never reported; counting begins again after the start.
/// Both fixture tasks are due from the first evaluation (the daily task's first run is owed, as
/// `gate_372k2` pins), so the client's lease holds both pairs from that evaluation on.
#[test]
fn held_t3_one_hold_is_reported_once_and_a_start_ends_the_holds_left_open() {
    let pairs = [HOST_HEAD, HOST_LATE];
    let held_ids = |since: u64| {
        BTreeSet::from(pairs.map(|task| held_signal_id(task, POLICY_INSTANCE_ALIAS, since)))
    };
    let reported = |cycle: &PolicyCycle| {
        cycle
            .task_held_signals
            .iter()
            .map(|signal| signal.signal_id.clone())
            .collect::<BTreeSet<_>>()
    };
    let mut slots = Slots::start(&held_sources(), at(0, 7, 0));
    let holder = slots.hold();
    for (hour, minute) in [(8, 0), (8, 25)] {
        assert!(
            slots
                .evaluate(at(0, hour, minute))
                .task_held_signals
                .is_empty(),
            "before the threshold ({hour}:{minute:02})"
        );
    }
    let cycle = slots.evaluate(at(0, 8, 30));
    assert_eq!(
        reported(&cycle),
        held_ids(at(0, 8, 0)),
        "both pairs, held since 08:00, at 08:30"
    );
    for minute in [35, 38] {
        assert!(
            slots
                .evaluate(at(0, 8, minute))
                .task_held_signals
                .is_empty(),
            "reported once (08:{minute})"
        );
    }
    let records = held_records(&slots.host);
    assert_eq!(records.len(), 2, "{records:#?}");
    let head_id = held_signal_id(HOST_HEAD, POLICY_INSTANCE_ALIAS, at(0, 8, 0));
    let (_, task_id, outcome, observed) = records
        .iter()
        .find(|(signal_id, ..)| *signal_id == head_id)
        .expect("the head's held record");
    assert_eq!(task_id.as_deref(), Some(HOST_HEAD));
    assert!(outcome.is(TaskHeldCode::Held), "{outcome:?}");
    assert_eq!(
        outcome.value("gate_code"),
        Some(&json!(HostCode::DispatchInstanceHeld.code().as_str()))
    );
    assert_eq!(*observed, at(0, 8, 30));

    // The start after it ends each open hold.
    Slots::release(holder);
    let mut slots = slots.restart(at(0, 8, 40));
    let records = held_records(&slots.host);
    assert_eq!(records.len(), 4, "{records:#?}");
    assert_eq!(
        records[2..]
            .iter()
            .map(|(signal_id, ..)| signal_id.clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(pairs.map(|task| ended_signal_id(task, POLICY_INSTANCE_ALIAS, at(0, 8, 0)))),
        "one end for each hold left open"
    );
    for (_, _, end, observed) in &records[2..] {
        assert!(end.is(TaskHeldCode::Ended), "{end:?}");
        assert_eq!(
            end.value("held_end"),
            Some(&json!(HeldEnd::Restarted.as_str()))
        );
        assert_eq!(end.value("held_since_unix_ms"), Some(&json!(at(0, 8, 0))));
        assert_eq!(
            end.value("gate_code"),
            Some(&json!(HostCode::DispatchInstanceHeld.code().as_str()))
        );
        assert!(
            end.value("held_for_ms").is_none(),
            "no held_for_ms for restarted"
        );
        assert_eq!(*observed, at(0, 8, 40), "observed at the start");
    }

    // A hold never reported leaves nothing at the start after it: held from 08:45, not yet past
    // the threshold at 08:55, where that run ends.
    let holder = slots.hold();
    for minute in [45, 55] {
        assert!(
            slots
                .evaluate(at(0, 8, minute))
                .task_held_signals
                .is_empty(),
            "before the threshold (08:{minute})"
        );
    }
    Slots::release(holder);
    let mut slots = slots.restart(at(0, 9, 0));
    assert_eq!(
        held_records(&slots.host).len(),
        4,
        "nothing for a hold never reported"
    );

    // Counting begins again: the holds of the new run start at its first evaluation.
    let holder = slots.hold();
    slots.evaluate(at(0, 9, 5));
    assert!(slots.evaluate(at(0, 9, 30)).task_held_signals.is_empty());
    let cycle = slots.evaluate(at(0, 9, 35));
    assert_eq!(
        reported(&cycle),
        held_ids(at(0, 9, 5)),
        "both pairs count from 09:05"
    );
    Slots::release(holder);
    slots.close();
}

/// The start's read of a held record, then a start that meets one it cannot read back. A
/// `gate_code` this build registers and does not hold still reads back (the held set is closed
/// per build, and a hold another build reported is ended all the same); a spelling this build's
/// catalog does not register, a missing start, or an id that is not the digest of the record's own
/// task, alias and start does not. Through the host: the start leaves such a record without an
/// end, still ends the holds it reads after it, and completes; so does the start after it, which
/// writes nothing more (register §1.327, settled by the coordinator on 11-12).
#[test]
fn held_t11_a_start_leaves_the_held_records_it_cannot_read_back_and_goes_on() {
    use crate::policy_host::held::{
        HeldRecordRead, ReportedHold, SignalView, reported_hold, unreadable_holds_line,
    };

    let since = at(0, 8, 0);
    let not_held = PolicyCode::TriggerFalse.code();
    let of_the_set = HostCode::DispatchInstanceHeld.code();
    let values = |gate_code: &str| {
        json!({
            "task_id": HOST_HEAD,
            "alias": POLICY_INSTANCE_ALIAS,
            "gate_code": gate_code,
            "held_state": HeldState::Due.as_str(),
            "held_since_unix_ms": since,
            "held_for_ms": DEFAULT.after_ms,
            "held_after_ms": DEFAULT.after_ms,
        })
    };
    let own_id = held_signal_id(HOST_HEAD, POLICY_INSTANCE_ALIAS, since);
    let read = |signal_id: &str, values: Value| {
        let outcome = why(TaskHeldCode::Held, values);
        reported_hold(&SignalView {
            signal_id,
            kind: PolicyPlanningSignalKind::GoalMissed,
            task_id: Some(HOST_HEAD),
            alias: POLICY_INSTANCE_ALIAS,
            outcome: &outcome,
        })
    };
    assert_eq!(
        read(&own_id, values(not_held.as_str())),
        HeldRecordRead::Held(ReportedHold {
            task_id: HOST_HEAD.to_owned(),
            alias: POLICY_INSTANCE_ALIAS.to_owned(),
            gate_code: not_held,
            since_unix_ms: since,
        }),
        "a registered code outside the held set reads back"
    );
    assert_eq!(
        read(&own_id, values("fixture_code_of_another_build")),
        HeldRecordRead::Unreadable,
        "a spelling this build's catalog does not register"
    );
    let mut without_start = values(of_the_set.as_str());
    without_start
        .as_object_mut()
        .expect("the values")
        .remove("held_since_unix_ms");
    assert_eq!(
        read(&own_id, without_start),
        HeldRecordRead::Unreadable,
        "no start"
    );
    assert_eq!(
        read(
            &held_signal_id(HOST_HEAD, POLICY_INSTANCE_ALIAS, since + MINUTE_MS),
            values(of_the_set.as_str())
        ),
        HeldRecordRead::Unreadable,
        "an id that is not the digest of the record's own start"
    );

    // The developer line: none when every record was read, else the first eight sequences and
    // the number of the rest.
    assert_eq!(unreadable_holds_line(&[]), None);
    assert_eq!(
        unreadable_holds_line(&[41, 57]).as_deref(),
        Some("actingd task_held unreadable sequences=41,57 more=0")
    );
    assert_eq!(
        unreadable_holds_line(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).as_deref(),
        Some("actingd task_held unreadable sequences=1,2,3,4,5,6,7,8 more=2")
    );

    // Through the host. The run before left three held records without an end: one whose id is
    // not the digest of its own start, one whose code this build registers and does not hold,
    // one of the held set.
    let slots = Slots::start(&held_sources(), at(0, 7, 0));
    let cause = trigger_false();
    let record = |task_id: &'static str, gate_code: Code, since_unix_ms: u64| {
        held_signal(
            &HeldRow {
                task_id,
                alias: POLICY_INSTANCE_ALIAS,
                why: &cause,
                gate_code,
                state: HeldState::Due,
                since_unix_ms,
                after_ms: DEFAULT.after_ms,
            },
            since_unix_ms + DEFAULT.after_ms,
        )
    };
    let mut other_digest = record(HOST_HEAD, of_the_set, since);
    other_digest.signal_id = held_signal_id(HOST_HEAD, POLICY_INSTANCE_ALIAS, since + MINUTE_MS);
    let other_set = record(HOST_LATE, not_held, since);
    let own_set = record(HOST_LATE, of_the_set, since + 2 * MINUTE_MS);
    for signal in [&other_digest, &other_set, &own_set] {
        slots
            .host
            .record_policy_planning_signal(signal.clone())
            .expect("record the held signal");
    }

    // The start completes: it leaves the first record as it is and ends the two it reads.
    let slots = slots.restart(at(0, 9, 0));
    let records = held_records(&slots.host);
    assert_eq!(
        records
            .iter()
            .map(|(signal_id, ..)| signal_id.clone())
            .collect::<Vec<_>>(),
        [
            other_digest.signal_id.clone(),
            other_set.signal_id.clone(),
            own_set.signal_id.clone(),
            ended_signal_id(HOST_LATE, POLICY_INSTANCE_ALIAS, since),
            ended_signal_id(HOST_LATE, POLICY_INSTANCE_ALIAS, since + 2 * MINUTE_MS),
        ],
        "{records:#?}"
    );
    for ((_, _, end, observed), gate_code) in records[3..].iter().zip([not_held, of_the_set]) {
        assert!(end.is(TaskHeldCode::Ended), "{end:?}");
        assert_eq!(
            end.value("held_end"),
            Some(&json!(HeldEnd::Restarted.as_str()))
        );
        assert_eq!(end.value("gate_code"), Some(&json!(gate_code.as_str())));
        assert_eq!(*observed, at(0, 9, 0), "observed at the start");
    }

    // That start recorded its anchor, so the one after it reads none of these records again:
    // it completes and writes nothing more.
    let slots = slots.restart(at(0, 9, 10));
    assert_eq!(held_records(&slots.host).len(), 5);
    slots.close();
}

/// With the record on and off, every evaluation decides the same (each decision's task,
/// instance, state, eligibility and reason codes), dispatches the same pairs and records the same
/// other planning signals; only the held records and their ends differ.
#[test]
fn held_t6_the_decisions_are_the_same_with_the_record_on_and_off() {
    let on = HeldHost::start(true, at(0, 7, 0));
    let off = HeldHost::start(false, at(0, 7, 0));
    let on_holder = on.hold();
    let off_holder = off.hold();
    for (hour, minute) in [(8, 0), (8, 25), (8, 30), (8, 35)] {
        let time = at(0, hour, minute);
        let (with, without) = (on.cycle(time), off.cycle(time));
        assert_eq!(
            decided(&with),
            decided(&without),
            "the same decisions at {hour}:{minute:02}"
        );
        assert!(
            without.task_held_signals.is_empty(),
            "no record with the switch off: {:#?}",
            without.task_held_signals
        );
        // Both pairs are due from the first evaluation (the daily task's first run is owed), so
        // the lease holds both from 08:00 and both are reported at 08:30.
        let expected = if (hour, minute) == (8, 30) {
            BTreeSet::from(
                [HOST_HEAD, HOST_LATE]
                    .map(|task| held_signal_id(task, POLICY_INSTANCE_ALIAS, at(0, 8, 0))),
            )
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            with.task_held_signals
                .iter()
                .map(|signal| signal.signal_id.clone())
                .collect::<BTreeSet<_>>(),
            expected,
            "{hour}:{minute:02}"
        );
    }
    release(on_holder);
    release(off_holder);
    let time = at(0, 8, 50);
    let (with, without) = (on.cycle(time), off.cycle(time));
    assert_eq!(
        decided(&with),
        decided(&without),
        "the same decisions at 8:50"
    );
    assert!(without.task_held_signals.is_empty());
    // Once the lease is released the head's reported hold ends; whatever the cycle writes is an
    // end of one of the two reported holds, never a new held record.
    let ends = with
        .task_held_signals
        .iter()
        .map(|signal| signal.signal_id.clone())
        .collect::<BTreeSet<_>>();
    let reported_ends = [HOST_HEAD, HOST_LATE]
        .map(|task| ended_signal_id(task, POLICY_INSTANCE_ALIAS, at(0, 8, 0)));
    assert!(
        ends.contains(&reported_ends[0]),
        "the head's reported hold ends once the lease is released: {ends:?}"
    );
    assert!(
        ends.iter().all(|id| reported_ends.contains(id)),
        "only ends of the two reported holds: {ends:?}"
    );
    on.host.close().expect("close the host with the record on");
    off.host
        .close()
        .expect("close the host with the record off");
}

/// A held record, its end and a start's end serialize with exactly the planning-signal key set,
/// decode and encode again to the same bytes, carry the warning and info severities of their
/// codes, come back from the projection as recorded, and an event page lists them by their ids.
#[test]
fn held_t8_the_records_round_trip_through_the_ledger_the_projection_and_an_event_page() {
    use crate::policy_host::held::{ReportedHold, restarted_end};

    let since = at(1, 6, 5);
    let cause = why(
        PolicyCode::PolicyNoProgress,
        json!({"served_by_dispatch_unix_ms": since - MINUTE_MS}),
    );
    let held = held_signal(
        &HeldRow {
            task_id: HOST_HEAD,
            alias: POLICY_INSTANCE_ALIAS,
            why: &cause,
            gate_code: PolicyCode::PolicyNoProgress.code(),
            state: HeldState::Due,
            since_unix_ms: since,
            after_ms: DEFAULT.after_ms,
        },
        since + DEFAULT.after_ms,
    );
    let ended = ended_signal(
        &EndedHold {
            task_id: HOST_HEAD,
            alias: POLICY_INSTANCE_ALIAS,
            gate_code: PolicyCode::PolicyNoProgress.code(),
            since_unix_ms: since,
            end: HeldEnd::Selected,
            held_for_ms: Some(DEFAULT.after_ms + MINUTE_MS),
        },
        since + DEFAULT.after_ms + MINUTE_MS,
    );
    let restarted = restarted_end(
        &ReportedHold {
            task_id: "fixture.held.other".to_owned(),
            alias: POLICY_INSTANCE_ALIAS.to_owned(),
            gate_code: PolicyCode::TriggerUnknown.code(),
            since_unix_ms: since,
        },
        since + DEFAULT.after_ms + 2 * MINUTE_MS,
    );
    let signals = [held, ended, restarted];
    let slots = Slots::start(&held_sources(), at(1, 6, 0));
    for signal in &signals {
        slots
            .host
            .record_policy_planning_signal(signal.clone())
            .expect("record the held signal");
    }
    let events = slots
        .host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        })
        .expect("planning signals")
        .into_iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
                    if payload.signal_id().starts_with(PREFIX)
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), signals.len());
    for ((event, signal), severity) in events.iter().zip(&signals).zip([
        EventSeverity::Warning,
        EventSeverity::Info,
        EventSeverity::Info,
    ]) {
        assert_eq!(event.severity(), severity, "{}", signal.signal_id);
        let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) = event.payload()
        else {
            panic!("a planning signal");
        };
        assert_eq!(payload.signal_id(), signal.signal_id);
        assert_eq!(payload.instance_id(), signal.instance_id);
        assert_eq!(payload.task_id(), signal.task_id.as_deref());
        assert_eq!(payload.outcome(), &signal.outcome);
        let bytes = serde_json::to_vec(payload).expect("encode the record");
        let record: Value = serde_json::from_slice(&bytes).expect("record JSON");
        let keys = record
            .as_object()
            .expect("a record object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "action",
                "signal_id",
                "instance_id",
                "task_id",
                "kind",
                "outcome",
                "observed_at_unix_ms",
                "audit",
            ])
        );
        let decoded: PolicyPlanningSignalPayload =
            serde_json::from_slice(&bytes).expect("decode the record");
        assert_eq!(
            serde_json::to_vec(&decoded).expect("encode the record again"),
            bytes
        );
    }
    let mut client = TestClient::connect(&slots.host);
    let page = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    let listed = page
        .iter()
        .filter_map(|event| {
            let signal_id = match &event.payload {
                ProjectionPayload::Public(public) => match public.as_ref() {
                    PublicEventPayload::Policy(policy) => {
                        policy.policy_signal_id().map(str::to_owned)
                    }
                    _ => None,
                },
                ProjectionPayload::Full(payload) => match &**payload {
                    EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal)) => {
                        Some(signal.signal_id().to_owned())
                    }
                    _ => None,
                },
                _ => None,
            }?;
            signal_id
                .starts_with(PREFIX)
                .then_some((signal_id, event.severity))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        signals
            .iter()
            .map(|signal| signal.signal_id.clone())
            .zip([
                EventSeverity::Warning,
                EventSeverity::Info,
                EventSeverity::Info
            ])
            .collect::<Vec<_>>(),
        "the event page lists the records"
    );
    drop(client);
    let root = slots.close_keeping_root();
    let state =
        RuntimeStateStore::open(root.path(), b"runtime-host-test-salt").expect("open the state");
    for signal in &signals {
        let (_, stored) = state
            .load_planning_signal(&signal.signal_id)
            .expect("read the projection")
            .expect("a projection row");
        assert_eq!(&stored, signal);
    }
}

/// `actingd suspended` lists under `held` each held record still open or ended within 48 h of the
/// ledger's latest event, newest first, with its end when one is recorded, every instance's; a hold
/// that ended earlier is not listed. (The diagnose half, the instance's rows and the mark on its
/// `not_run` rows, is lab-ops' `held_t9`.)
#[test]
fn held_t9_actingd_suspended_and_diagnose_list_an_instance_held_records() {
    const OPEN: &str = "fixture.held.open";
    const ENDED: &str = "fixture.held.ended";
    const OLD: &str = "fixture.held.old";
    const ELSEWHERE: &str = "fixture.held.elsewhere";
    let cause = why(PolicyCode::InstanceUnavailable, json!({}));
    let held_at = |task_id: &str, alias: &str, since: u64, at: u64| {
        held_signal(
            &HeldRow {
                task_id,
                alias,
                why: &cause,
                gate_code: PolicyCode::InstanceUnavailable.code(),
                state: HeldState::Due,
                since_unix_ms: since,
                after_ms: DEFAULT.after_ms,
            },
            at,
        )
    };
    let ended_at = |task_id: &str, since: u64, at: u64| {
        ended_signal(
            &EndedHold {
                task_id,
                alias: POLICY_INSTANCE_ALIAS,
                gate_code: PolicyCode::InstanceUnavailable.code(),
                since_unix_ms: since,
                end: HeldEnd::Selected,
                held_for_ms: Some(at - since),
            },
            at,
        )
    };
    let now = POLICY_NOW_UNIX_MS;
    let old_held = held_at(OLD, POLICY_INSTANCE_ALIAS, now, now + 30 * MINUTE_MS);
    let old_end = ended_at(OLD, now, now + HOUR_MS);
    let open = held_at(
        OPEN,
        POLICY_INSTANCE_ALIAS,
        now + HOUR_MS,
        now + 2 * HOUR_MS,
    );
    let ended_held = held_at(
        ENDED,
        POLICY_INSTANCE_ALIAS,
        now + 2 * HOUR_MS,
        now + 3 * HOUR_MS,
    );
    let ended = ended_at(ENDED, now + 2 * HOUR_MS, now + 4 * HOUR_MS);
    // Latest: more than 48 h after the old hold's end.
    let latest = now + 49 * HOUR_MS + MINUTE_MS;
    let elsewhere = held_at(
        ELSEWHERE,
        POLICY_INSTANCE_ALIAS_B,
        latest - DEFAULT.after_ms,
        latest,
    );
    let signals = [old_held, old_end, open, ended_held, ended, elsewhere];
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(now, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let sources = policy_sources(1);
    host.activate_policy_catalog(&sources)
        .expect("activate the catalog");
    for signal in &signals {
        advance_manual_clock_to(&clock, signal.observed_at_unix_ms);
        host.record_policy_planning_signal(signal.clone())
            .expect("append the held signal");
    }
    host.close().expect("close the runtime host");
    let (document, events) = (report(&root, &sources), ledger_events(&root));
    let entry = |signal: &PolicyPlanningSignalEventData| {
        let held_since = signal.outcome.value("held_since_unix_ms").cloned();
        json!({
            "code": "task_held",
            "task_id": signal.task_id,
            "instance_id": signal.instance_id,
            "gate_code": "instance_unavailable",
            "held_state": "due",
            "held_since_unix_ms": held_since,
            "held_for_ms": signal.outcome.value("held_for_ms").cloned(),
            "held_after_ms": DEFAULT.after_ms,
            "recorded_sequence": record_sequence(&events, &signal.signal_id),
            "recorded_at_unix_ms": signal.observed_at_unix_ms,
        })
    };
    let mut ended_entry = entry(&signals[3]);
    ended_entry["ended"] = json!({
        "sequence": record_sequence(&events, &signals[4].signal_id),
        "at_unix_ms": signals[4].observed_at_unix_ms,
        "held_end": "selected",
        "held_for_ms": 2 * HOUR_MS,
    });
    assert_eq!(
        document["held"],
        json!([entry(&signals[5]), ended_entry, entry(&signals[2])]),
        "newest first; the hold that ended more than 48 h before is not listed: {document:#}"
    );
    assert!(document.get("held_total").is_none(), "{document:#}");
}
