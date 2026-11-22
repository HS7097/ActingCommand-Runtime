// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19a (register §1.143 M19A-2; `contracts/scheduling/README.md`, "Why
//! A Candidate Does Not Run"): the latest evaluation's not-run explanation, per instance, kept in
//! the policy host's memory. Every evaluated cycle replaces it from the bound evaluation, before
//! the host adds its notes to the decisions; a deferred cycle and a failed evaluation leave it as
//! it was, and a start begins with none. Nothing here reaches the ledger and no decision reads
//! it: only the read `ScheduleExplanation` and the idle check (IDLE-1, its plan per instance) do,
//! and (Workflow #438 HELD-1) the held records, which read its rows after every evaluated cycle
//! (`policy_host::held`). From HELD-1 the three waits for one fact's refresh of one `fact_key`
//! count as one `why` code for a row's `since_unix_ms` (`same_clock`).

use crate::application_update::IdleUpdateHold;
use crate::idle_action::IdlePlan;
use actingcommand_contract::codes::ScheduleExplanationRowCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    MAX_SCHEDULE_EXPLANATION_CANDIDATES, MAX_SCHEDULE_EXPLANATION_REASONS,
    RuntimeScheduleExplanation, ScheduleCandidateEligibility, ScheduleCandidateState,
    ScheduleDispatchState, ScheduleExplanationCandidate, ScheduleExplanationInstance,
    ScheduleExplanationNotRunning, ScheduleExplanationSelected,
};
use actingcommand_policy::{
    ActivityProfile, Blackout, EligibilityState, FACT_REFRESH_TASK_ID, PolicyCode,
    PolicyEvaluation, PolicyEvaluationResult, SchedulingDecisionState, TaskDecision,
    activity_window_at,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// IDLE-WG (0.12.27): by instance alias, the instances a time-bound gate holds back as an
/// evaluation runs: a blackout covers the instance, or its activity window is closed. Each with
/// the time the gates first let a candidate of it through (the blackout's end, then the next
/// opening of its activity window); none when its activity window does not open within the
/// eight days its windows are searched.
pub(crate) type IdleHeldBack = BTreeMap<String, Option<u64>>;

/// #484 IDLE-W2 (0.12.75; register §1.488): by instance alias, then task id, the candidates of an
/// evaluation that a hold of their instance deferred (positions 1 and 2 of the order of the
/// deferrals: a control gate such as a pause, a claim on its key, a cooldown, a wait for another
/// member of its device) while a later position would hold them back as well, each with the time
/// that position lets it through (none: it names no time). The policy host reads the later
/// positions for the idle plan alone (`PolicyHost::evaluate`, its look ahead); a candidate no later
/// position holds back is not here, and nothing a dispatch decides reads it. Fix round 2: here
/// too, with no time, is each candidate of an instance the idle action closed that the evaluation
/// blocked because the instance is unavailable while such a hold holds it (a pause a start
/// restored, which skipped its preparation, until the resume's reconnect proves it again).
pub(crate) type IdleHeldAhead = BTreeMap<String, BTreeMap<String, Option<u64>>>;

/// What the idle plan reads beside an evaluation, which only the idle plan reads; the rows stay the
/// evaluation's own: the instances a time-bound gate holds back (IDLE-WG, [`IdleHeldBack`]), the
/// candidates a hold of their instance hides a later gate of (#484 IDLE-W2, [`IdleHeldAhead`]) and
/// the instances position 4a holds (UPD-1 fix round 1, review M1).
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdleGates<'a> {
    pub(crate) held_back: &'a IdleHeldBack,
    pub(crate) held_ahead: &'a IdleHeldAhead,
    pub(crate) update_hold: IdleUpdateHold<'a>,
}

/// #486 ASLEEP-1 fix round 1 and ASLEEP-2 (register §1.506, rulings 2 and 7): what the idle plan of
/// an instance that its idle action can put to sleep reads beside an evaluation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdleSleepers<'a> {
    /// The instances whose idle action in effect can stop their emulator or close their
    /// application, by alias (`HostShared::idle_sleepers_for_policy`).
    pub(crate) instances: &'a BTreeSet<String>,
    /// Ruling 7: the next projection step of each regenerating pool the evaluation read
    /// (`actingcommand_policy::pool_projection_steps`). A decision's wake that is one of them is
    /// the evaluation's own next reading of that pool, no time a task is due, so on such an
    /// instance it is no wake of the plan. A wake a time names (a clock or timeline time, a
    /// cooldown's end, a budget's release, an activity window's opening, a deadline) is another
    /// time and stays.
    pub(crate) pool_steps: &'a BTreeSet<u64>,
}

const DAY_MS: i128 = 86_400_000;
const MINUTE_MS: i128 = 60_000;

/// IDLE-WG (0.12.27): the instances a time-bound gate holds back at `now_unix_ms` (see
/// [`IdleHeldBack`]), from each instance's activity profile (`instances`: its alias and the
/// profile the evaluator matches to it; none: no window to judge) and the blackouts covering
/// instances now (`blackouts`, by alias, as the cycle judged them for position 4). A gate that a
/// hold of the instance (a pause, a claim on its key) comes before still holds the candidate back:
/// the idle plan reads it here, not from the decision's reason, which names the first gate only.
pub(crate) fn idle_held_back<'a>(
    instances: impl IntoIterator<Item = (&'a str, Option<&'a ActivityProfile>)>,
    blackouts: &BTreeMap<String, Blackout>,
    now_unix_ms: u64,
) -> PolicyEvaluationResult<IdleHeldBack> {
    let mut held_back = IdleHeldBack::new();
    for (alias, profile) in instances {
        let covering = blackouts.get(alias);
        let clear_from = covering.map_or(now_unix_ms, |blackout| {
            blackout.end_unix_ms.max(now_unix_ms)
        });
        let window_open = match profile {
            Some(profile) => activity_window_at(profile, clear_from)?.is_some(),
            None => true,
        };
        if covering.is_none() && window_open {
            continue;
        }
        let through = match profile {
            Some(profile) if !window_open => next_window_opening(profile, clear_from)?,
            _ => Some(clear_from),
        };
        held_back.insert(alias.to_owned(), through);
    }
    Ok(held_back)
}

/// IDLE-WG: the first time after `after_unix_ms` at which `profile` has an open activity window,
/// searched as the admission's own boundary search does (one week and the following day of each
/// window's local midnights and starts); none when no window opens in that span.
fn next_window_opening(
    profile: &ActivityProfile,
    after_unix_ms: u64,
) -> PolicyEvaluationResult<Option<u64>> {
    let mut openings = BTreeSet::new();
    for window in &profile.windows {
        let offset = i128::from(window.utc_offset_minutes) * MINUTE_MS;
        let local_day = (i128::from(after_unix_ms) + offset).div_euclid(DAY_MS);
        for day in local_day..=local_day + 8 {
            for minute in [0, window.start_minute_of_day] {
                let opening = day * DAY_MS + i128::from(minute) * MINUTE_MS - offset;
                if let Ok(opening) = u64::try_from(opening)
                    && opening > after_unix_ms
                {
                    openings.insert(opening);
                }
            }
        }
    }
    for opening in openings {
        if activity_window_at(profile, opening)?.is_some() {
            return Ok(Some(opening));
        }
    }
    Ok(None)
}

/// SCH-M ED-2 (§3.4 "Visible"): for a (task id, instance alias), the duration its dispatch would
/// reserve and where it comes from (`declared` or `runs:<n>`).
pub(crate) type ExpectedRuntimeOf<'a> = dyn Fn(&str, &str) -> Option<(u64, String)> + 'a;

/// The latest evaluated cycle's explanation; `None` until the first one after a start.
#[derive(Debug, Default)]
pub(crate) struct ScheduleExplanationTable {
    latest: Option<Explanation>,
}

#[derive(Debug)]
struct Explanation {
    evaluated_at_unix_ms: u64,
    catalog_hash: String,
    /// By instance alias.
    instances: BTreeMap<String, InstanceRows>,
}

#[derive(Debug)]
struct InstanceRows {
    next_wake_unix_ms: Option<u64>,
    /// The selected decision's task and decision id.
    selected: Option<(String, String)>,
    /// Every candidate row, in answer order; the read cuts them.
    candidates: Vec<ScheduleExplanationCandidate>,
    not_running: ScheduleExplanationNotRunning,
    /// IDLE-1 (IDLE-1P, #405): the instance's plan as the idle check reads it.
    idle: IdlePlan,
    /// #484 IDLE-W2: the task the plan counts due, which a wake for it names: the selected one,
    /// else the first due one in the evaluation's order; none when no task is due.
    idle_due_task: Option<String>,
}

/// IDLE-1P (#405): one instance's plan as the idle check reads it, gathered over its decisions.
/// A decision is due when it is selected, or eligible with no wake of its own later than the
/// evaluation (held by the instance's queue, blocked); an eligible one deferred to a later time
/// (a budget, a random delay, a gate's wake) is due at that wake. The fact-refresh reading is
/// kept apart from the tasks.
///
/// IDLE-WG (0.12.27; production v0.12.26 on 10-29 02:43: an instance resume's reconnect claim
/// held an idle instance, its candidates waited `dispatch_instance_held` with no wake, and the
/// plan woke the closed game for tasks its closed activity window then refused): a task is due
/// only when it could be dispatched now. On an instance a time-bound gate holds back
/// ([`IdleHeldBack`]: a blackout covers it, or its activity window is closed) no unselected task
/// is due, whatever gate its decision names first (a hold of the instance comes before both); its
/// wake is the time the gates let it through, not its own.
///
/// #484 IDLE-W2 (0.12.75; production 0.12.74 on 11-19 08:00: an application the idle check had
/// closed was woken half a second later, seven times in twelve minutes, while the close's own claim
/// or a Lab run held the instance's key and the only candidate's budget was spent): so is a task
/// that a hold of the instance (a pause, a claim on its key) defers while a later position of the
/// order would hold it back as well ([`IdleHeldAhead`]: a spent budget, a backoff, the activity
/// interval, a suspended package among them). It is not due, and its wake is that position's
/// release; with a time-bound gate as well, the later of the two. A task no later position holds
/// back is due as before, also behind a hold: it wakes the instance at once. Fix round 1
/// (review-1845): a hold with a time of its own (a cooldown) lets such a task through no earlier
/// than that time, so its wake is the later of the cooldown's end and the later position's
/// release, none when either names none.
///
/// #486 ASLEEP-1 fix round 1 (register §1.497; `contracts/runtime-config.md`, "What wakes an idle
/// instance"): on an instance whose idle action in effect can stop its emulator (`stop_emulator`,
/// and `auto` where that tier is allowed: `HostShared::idle_sleepers_for_policy`), a decision
/// a gate without a time holds (the benefit-held set below) contributes no wake either, whatever
/// next time its trigger names: that time would start a stopped emulator when nothing can be
/// dispatched. Nothing is lost but the lead: every evaluation judges an asleep instance as an
/// available one, so the evaluation in which the task can be dispatched counts it due. This holds
/// for the plan the stop is decided by too, since a wake only ever moves earlier. #486 ASLEEP-2
/// (register §1.506, ruling 2): so does an instance whose idle action in effect can close its
/// application (`close_app`, and `auto` at any tier, its startup package in effect: the same
/// set), whose closed application the time would start for nothing. On every other instance (its
/// idle action `none`, or none it can take) the plan is as before.
#[derive(Debug, Default)]
struct IdleDraft {
    task_due: bool,
    task_wake: Option<u64>,
    reading_due: Option<u64>,
    /// #484 IDLE-W2: the task a wake names when one is due, and whether it was selected: the
    /// selected one, else the first due one in the evaluation's order.
    due_task: Option<(String, bool)>,
}

impl IdleDraft {
    /// `held_back`: the instance's entry of [`IdleHeldBack`], when a time-bound gate holds it.
    /// `held_ahead`: the decision's entry of [`IdleHeldAhead`], when a hold of the instance hides
    /// a later position that holds it back. `update_held`: the instance waits for its client update
    /// and the decision is no store update task's
    /// ([`crate::application_update::IdleUpdateHold::holds`]). `sleeper` (#486 ASLEEP-1 fix round
    /// 1; ASLEEP-2, rulings 2 and 7): when the instance's idle action in effect can stop its
    /// emulator or close its application, the next projection steps of the regenerating pools the
    /// evaluation read ([`IdleSleepers::pool_steps`]); none on any other instance.
    fn add(
        &mut self,
        decision: &TaskDecision,
        evaluated_at_unix_ms: u64,
        held_back: Option<Option<u64>>,
        held_ahead: Option<Option<u64>>,
        update_held: bool,
        sleeper: Option<&BTreeSet<u64>>,
    ) {
        // UPD-1 (design §4.2; fix round 1, review M1): on an instance that waits for its client
        // update, a decision other than the store update task's is neither due nor a wake,
        // whatever its reason (a cooldown or a clock trigger not come included: the gate holds it
        // once it is a candidate). So the plan never wakes the instance for it (its startup
        // package would only meet the update dialog again); the store update task's own decision,
        // its cooldown's wake included, still counts.
        if update_held {
            return;
        }
        // SS-2d (register §1.235): a decision within its rounding error (released or deferred,
        // `benefit_within_tolerance` among its reasons) is neither due nor a wake of its own
        // unless it was selected: a zero tier only fills an open game's idle time, and never wakes
        // a closed one for work worth 0.
        if decision.state != SchedulingDecisionState::Selected
            && decision
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::BenefitWithinTolerance))
        {
            return;
        }
        let eligible = decision.state == SchedulingDecisionState::Selected
            || decision.eligibility == EligibilityState::True;
        // SS-2b (register §1.225): a judged candidate the benefit gate holds (its net benefit
        // unknown or not positive) cannot be dispatched now, so it is never due; only a wake of
        // its own counts. Without it every business task waiting for its values would keep a
        // closed game awake.
        // SS-4 (register §1.231): so is a pair the no-progress guard holds, which waits for a
        // reading of another value and has no wake of its own.
        // SS-2h (register §1.289): and a judged candidate a pool's lines hold (its hard line, or
        // a capped pool's order that another consumer heads); SCHED-C1c (register §1.299): or
        // whose pool its lines cannot spend.
        let benefit_held = decision.reasons.last().is_some_and(|reason| {
            reason.outcome.is(PolicyCode::BenefitUnknown)
                || reason.outcome.is(PolicyCode::PolicyBenefitNonpositive)
                || reason.outcome.is(PolicyCode::PolicyNoProgress)
                || reason.outcome.is(PolicyCode::PoolLineHeld)
                || reason.outcome.is(PolicyCode::PoolLinesUnspendable)
        });
        let due = decision.state == SchedulingDecisionState::Selected
            || (eligible
                && !benefit_held
                && decision
                    .next_wake_unix_ms
                    .is_none_or(|wake| wake <= evaluated_at_unix_ms));
        if decision.task_id != FACT_REFRESH_TASK_ID
            && decision.state != SchedulingDecisionState::Selected
            && due
            && let Some(through) = held_until(held_back, held_ahead)
        {
            // IDLE-WG, #484 IDLE-W2: not dispatchable now; it wakes the instance when what holds
            // it back lets it through.
            self.task_wake = earliest(self.task_wake, through);
            return;
        }
        // #484 IDLE-W2 fix round 1 (review-1845's must-fix): a candidate a hold with a time of its
        // own defers (a cooldown) is not due before that time; when a later position would hold it
        // back as well, it is let through at the later of the two times, and none when either
        // names none. Its own time alone moved a closed instance's wake to the cooldown's end: a
        // start for nothing.
        if decision.task_id != FACT_REFRESH_TASK_ID
            && !due
            && let Some(release) = held_ahead
        {
            self.task_wake = earliest(
                self.task_wake,
                decision
                    .next_wake_unix_ms
                    .zip(release)
                    .map(|(own, later)| own.max(later)),
            );
            return;
        }
        if decision.task_id == FACT_REFRESH_TASK_ID {
            let at = if due {
                Some(evaluated_at_unix_ms)
            } else if eligible {
                decision.next_wake_unix_ms
            } else {
                None
            };
            self.reading_due = earliest(self.reading_due, at);
        } else {
            self.task_due |= due;
            // #486 ASLEEP-1 fix round 1: on an instance whose idle action can stop its emulator
            // (#486 ASLEEP-2, ruling 2: or close its application), a decision a gate without a
            // time holds names no wake, whatever its trigger's next time. #486 ASLEEP-2, ruling 7:
            // nor does one whose wake is only the evaluation's next reading of a regenerating
            // pool (a pool's next projection step, whatever the decision's code): no task is due
            // then, and the evaluation in which the candidate can be dispatched wakes the
            // instance for it.
            let wakes_nothing = sleeper.is_some_and(|pool_steps| {
                benefit_held
                    || decision
                        .next_wake_unix_ms
                        .is_some_and(|wake| pool_steps.contains(&wake))
            });
            if !wakes_nothing {
                self.task_wake = earliest(self.task_wake, decision.next_wake_unix_ms);
            }
            if due {
                let selected = decision.state == SchedulingDecisionState::Selected;
                if self
                    .due_task
                    .as_ref()
                    .is_none_or(|(_, named_selected)| selected && !named_selected)
                {
                    self.due_task = Some((decision.task_id.clone(), selected));
                }
            }
        }
    }

    const fn plan(&self, evaluated_at_unix_ms: u64) -> IdlePlan {
        IdlePlan {
            evaluated_at_unix_ms,
            due_now: self.task_due,
            next_wake_unix_ms: self.task_wake,
            reading_due_unix_ms: self.reading_due,
        }
    }
}

/// One unselected decision with its rank, before the rows are ordered.
struct Draft<'a> {
    decision: &'a TaskDecision,
    why: &'a Outcome,
}

impl ScheduleExplanationTable {
    /// `record_held_back` with no instance held back and none waiting for its client update.
    #[cfg(test)]
    pub(crate) fn record(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
        expected: &ExpectedRuntimeOf<'_>,
    ) {
        self.record_held_back(
            evaluation,
            catalog_hash,
            evaluated_at_unix_ms,
            expected,
            &IdleHeldBack::new(),
            IdleUpdateHold::default(),
        );
    }

    /// `record_gated` with no candidate whose hold hides a later gate (#484 IDLE-W2).
    #[cfg(test)]
    pub(crate) fn record_held_back(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
        expected: &ExpectedRuntimeOf<'_>,
        held_back: &IdleHeldBack,
        update_hold: IdleUpdateHold<'_>,
    ) {
        self.record_gated(
            evaluation,
            catalog_hash,
            evaluated_at_unix_ms,
            expected,
            IdleGates {
                held_back,
                held_ahead: &IdleHeldAhead::new(),
                update_hold,
            },
        );
    }

    /// Replaces the table from an evaluated cycle's bound evaluation (`catalog_hash` its
    /// catalog's, `evaluated_at_unix_ms` its time). A row's `since_unix_ms` carries over from the
    /// table it replaces while the same (instance, task) keeps the same `why` code, and the
    /// not-running row's while the same (instance, task or none) keeps it; otherwise it is this
    /// evaluation's time. Workflow #438 HELD-1: the three waits for one fact's refresh that name
    /// the same `fact_key` count as one code (`same_clock`). SCH-M ED-2 (§3.4 "Visible"): `expected` gives, for a (task, instance
    /// alias), the duration its dispatch would reserve and where it comes from (`declared` or
    /// `runs:<n>`); a row carries both. `gates`, which only the idle plan reads (the rows stay the
    /// evaluation's own): IDLE-WG (0.12.27), the instances a time-bound gate holds back as the
    /// evaluation ran ([`idle_held_back`]); #484 IDLE-W2 (0.12.75), the candidates a hold of their
    /// instance deferred while a later position would hold them back ([`IdleHeldAhead`]); UPD-1 fix
    /// round 1 (review M1), the instances position 4a holds as the evaluation ran and the store
    /// update tasks it lets through.
    #[cfg(test)]
    pub(crate) fn record_gated(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
        expected: &ExpectedRuntimeOf<'_>,
        gates: IdleGates<'_>,
    ) {
        self.record_stopping(
            evaluation,
            catalog_hash,
            evaluated_at_unix_ms,
            expected,
            gates,
            IdleSleepers {
                instances: &BTreeSet::new(),
                pool_steps: &BTreeSet::new(),
            },
        );
    }

    /// `record_gated` with, #486 ASLEEP-1 fix round 1 (register §1.497), `sleepers`: the
    /// instances whose idle action in effect can stop their emulator (#486 ASLEEP-2, register
    /// §1.506, ruling 2: or close their application), whose idle plan takes no wake from a decision
    /// a gate without a time holds, nor (ruling 7) from one whose wake is only a regenerating
    /// pool's next projection step ([`IdleSleepers`], [`IdleDraft`]). Only the idle plan reads it.
    pub(crate) fn record_stopping(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
        expected: &ExpectedRuntimeOf<'_>,
        gates: IdleGates<'_>,
        sleepers: IdleSleepers<'_>,
    ) {
        let previous = self.latest.take();
        let mut wakes = BTreeMap::<&str, Option<u64>>::new();
        let mut selected = BTreeMap::<&str, (String, String)>::new();
        let mut drafts = BTreeMap::<&str, Vec<Draft<'_>>>::new();
        let mut idle = BTreeMap::<&str, IdleDraft>::new();
        for decision in &evaluation.decisions {
            let Some(alias) = decision.instance_id.as_deref() else {
                continue;
            };
            let wake = wakes.entry(alias).or_default();
            *wake = earliest(*wake, decision.next_wake_unix_ms);
            idle.entry(alias).or_default().add(
                decision,
                evaluated_at_unix_ms,
                gates.held_back.get(alias).copied(),
                gates
                    .held_ahead
                    .get(alias)
                    .and_then(|tasks| tasks.get(&decision.task_id))
                    .copied(),
                gates.update_hold.holds(decision),
                sleepers
                    .instances
                    .contains(alias)
                    .then_some(sleepers.pool_steps),
            );
            let rows = drafts.entry(alias).or_default();
            if decision.state == SchedulingDecisionState::Selected {
                // Every selected decision has its intent (bound to the same decision id the
                // dispatch projection keys).
                if let Some(intent) = evaluation.dispatch_intents.iter().find(|intent| {
                    intent.task_id == decision.task_id && intent.instance_id == alias
                }) {
                    selected.insert(alias, (intent.task_id.clone(), intent.decision_id.clone()));
                }
                continue;
            }
            // The evaluator gives every unselected decision of an instance a reason
            // (`decision_reason_missing` when nothing settled it), so each has a why.
            let Some(last) = decision.reasons.last() else {
                continue;
            };
            rows.push(Draft {
                decision,
                why: &last.outcome,
            });
        }
        let mut instances = BTreeMap::new();
        for (alias, mut rows) in drafts {
            rows.sort_by(answer_order);
            let earlier = previous
                .as_ref()
                .and_then(|explanation| explanation.instances.get(alias));
            let candidates = rows
                .iter()
                .map(|draft| {
                    candidate_row(
                        draft,
                        earlier,
                        evaluated_at_unix_ms,
                        expected(&draft.decision.task_id, alias),
                    )
                })
                .collect::<Vec<_>>();
            let next_wake_unix_ms = wakes.get(alias).copied().flatten();
            let not_running = not_running_row(
                alias,
                &candidates,
                next_wake_unix_ms,
                earlier.map(|rows| &rows.not_running),
                evaluated_at_unix_ms,
            );
            let draft = idle.remove(alias).unwrap_or_default();
            instances.insert(
                alias.to_owned(),
                InstanceRows {
                    next_wake_unix_ms,
                    selected: selected.remove(alias),
                    candidates,
                    not_running,
                    idle: draft.plan(evaluated_at_unix_ms),
                    idle_due_task: draft.due_task.map(|(task_id, _)| task_id),
                },
            );
        }
        self.latest = Some(Explanation {
            evaluated_at_unix_ms,
            catalog_hash: catalog_hash.to_owned(),
            instances,
        });
    }

    /// Workflow #438 HELD-1 (`contracts/scheduling/README.md`, "Held records"): the latest
    /// evaluated cycle's candidate rows, by instance alias, every instance the evaluation decided
    /// something for (one whose only decision was selected has none); `None` before the first
    /// evaluated cycle after a start. The held records read them after the table is replaced.
    pub(crate) fn candidate_rows(&self) -> Option<BTreeMap<&str, &[ScheduleExplanationCandidate]>> {
        let latest = self.latest.as_ref()?;
        Some(
            latest
                .instances
                .iter()
                .map(|(alias, rows)| (alias.as_str(), rows.candidates.as_slice()))
                .collect(),
        )
    }

    /// IDLE-1 (IDLE-1P, #405): the latest evaluation's plan for `alias` as the idle check reads
    /// it: none before the first evaluated cycle after a start; an instance the evaluation decided
    /// nothing for has no due task, no wake and no reading.
    pub(crate) fn idle_plan(&self, alias: &str) -> Option<IdlePlan> {
        let latest = self.latest.as_ref()?;
        Some(latest.instances.get(alias).map_or_else(
            || IdleDraft::default().plan(latest.evaluated_at_unix_ms),
            |rows| rows.idle,
        ))
    }

    /// #484 IDLE-W2: the task the latest evaluation's plan for `alias` counts due, which a wake for
    /// it names (the selected one, else the first due one in the evaluation's order); none when
    /// none is due or before the first evaluated cycle after a start.
    pub(crate) fn idle_due_task(&self, alias: &str) -> Option<&str> {
        self.latest
            .as_ref()?
            .instances
            .get(alias)?
            .idle_due_task
            .as_deref()
    }

    /// The answer of `ScheduleExplanation`: the entry of `instance` (none when the latest
    /// evaluation had no decision for it), else every entry, by alias; candidate rows only when
    /// `candidates` asks for them, at most [`MAX_SCHEDULE_EXPLANATION_CANDIDATES`] per entry.
    /// `dispatch_state` reads where a selected decision's dispatch stands now.
    pub(crate) fn answer(
        &self,
        instance: Option<&str>,
        candidates: bool,
        dispatch_state: impl Fn(&str) -> ScheduleDispatchState,
    ) -> RuntimeScheduleExplanation {
        let Some(latest) = &self.latest else {
            return RuntimeScheduleExplanation {
                evaluated_at_unix_ms: None,
                catalog_hash: None,
                instances: Vec::new(),
            };
        };
        let instances = latest
            .instances
            .iter()
            .filter(|(alias, _)| instance.is_none_or(|wanted| wanted == alias.as_str()))
            .map(|(alias, rows)| ScheduleExplanationInstance {
                instance_alias: alias.clone(),
                next_wake_unix_ms: rows.next_wake_unix_ms,
                selected: rows.selected.as_ref().map(|(task_id, decision_id)| {
                    ScheduleExplanationSelected {
                        task_id: task_id.clone(),
                        decision_id: decision_id.clone(),
                        dispatch_state: dispatch_state(decision_id),
                    }
                }),
                not_running: rows.not_running.clone(),
                candidates: if candidates {
                    rows.candidates
                        .iter()
                        .take(MAX_SCHEDULE_EXPLANATION_CANDIDATES)
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                },
                // A catalog holds at most `MAX_TASKS` (4 096) tasks.
                candidates_total: u32::try_from(rows.candidates.len()).unwrap_or(u32::MAX),
            })
            .collect();
        RuntimeScheduleExplanation {
            evaluated_at_unix_ms: Some(latest.evaluated_at_unix_ms),
            catalog_hash: Some(latest.catalog_hash.clone()),
            instances,
        }
    }
}

/// Due rows (eligibility true) first: ranked ones by total score, highest first, then unranked
/// ones, each tie by task id; then the rest by their wake, earliest first and none last, then by
/// task id.
fn answer_order(left: &Draft<'_>, right: &Draft<'_>) -> Ordering {
    let (left, right) = (left.decision, right.decision);
    match (due_of(left), due_of(right)) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let by_place = if due_of(left) {
        match (&left.rank, &right.rank) {
            (Some(left_rank), Some(right_rank)) => {
                right_rank.total_score.cmp(&left_rank.total_score)
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    } else {
        match (left.next_wake_unix_ms, right.next_wake_unix_ms) {
            (Some(left_wake), Some(right_wake)) => left_wake.cmp(&right_wake),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    };
    by_place.then_with(|| left.task_id.cmp(&right.task_id))
}

fn due_of(decision: &TaskDecision) -> bool {
    decision.eligibility == EligibilityState::True
}

fn candidate_row(
    draft: &Draft<'_>,
    earlier: Option<&InstanceRows>,
    evaluated_at_unix_ms: u64,
    expected: Option<(u64, String)>,
) -> ScheduleExplanationCandidate {
    let decision = draft.decision;
    let omitted = decision
        .reasons
        .len()
        .saturating_sub(MAX_SCHEDULE_EXPLANATION_REASONS);
    let since_unix_ms = earlier
        .and_then(|rows| {
            rows.candidates
                .iter()
                .find(|row| row.task_id == decision.task_id)
        })
        .filter(|row| same_clock(&row.why, draft.why))
        .map_or(evaluated_at_unix_ms, |row| row.since_unix_ms);
    ScheduleExplanationCandidate {
        task_id: decision.task_id.clone(),
        state: match decision.state {
            SchedulingDecisionState::Blocked => ScheduleCandidateState::Blocked,
            // An eligible decision that another task outranked waited this round.
            SchedulingDecisionState::Deferred
            | SchedulingDecisionState::Eligible
            | SchedulingDecisionState::Selected => ScheduleCandidateState::Deferred,
        },
        eligibility: match decision.eligibility {
            EligibilityState::True => ScheduleCandidateEligibility::True,
            EligibilityState::False => ScheduleCandidateEligibility::False,
            EligibilityState::Unknown => ScheduleCandidateEligibility::Unknown,
        },
        why: draft.why.clone(),
        reasons: decision.reasons[omitted..]
            .iter()
            .map(|reason| reason.outcome.code().clone())
            .collect(),
        // A decision holds at most a reason chain's bounded number of reasons.
        reasons_omitted: (omitted > 0).then(|| u32::try_from(omitted).unwrap_or(u32::MAX)),
        next_wake_unix_ms: decision.next_wake_unix_ms,
        since_unix_ms,
        expected_runtime_ms: expected.as_ref().map(|(expected_ms, _)| *expected_ms),
        expected_runtime_source: expected.map(|(_, source)| source),
    }
}

/// The instance's not-running row: its first due candidate row, else `due_task_missing` with
/// the instance's earliest wake.
fn not_running_row(
    alias: &str,
    candidates: &[ScheduleExplanationCandidate],
    next_wake_unix_ms: Option<u64>,
    earlier: Option<&ScheduleExplanationNotRunning>,
    evaluated_at_unix_ms: u64,
) -> ScheduleExplanationNotRunning {
    let (task_id, why, row_wake) = match candidates
        .first()
        .filter(|row| row.eligibility == ScheduleCandidateEligibility::True)
    {
        Some(row) => (
            Some(row.task_id.clone()),
            row.why.clone(),
            row.next_wake_unix_ms,
        ),
        None => {
            let why = Outcome::new(ScheduleExplanationRowCode::DueTaskMissing).instance_id(alias);
            let why = match next_wake_unix_ms {
                Some(wake) => why.next_eligible_unix_ms(wake),
                None => why,
            };
            (None, why, None)
        }
    };
    let since_unix_ms = earlier
        .filter(|row| row.task_id == task_id && same_clock(&row.why, &why))
        .map_or(evaluated_at_unix_ms, |row| row.since_unix_ms);
    ScheduleExplanationNotRunning {
        task_id,
        why,
        next_wake_unix_ms: row_wake,
        since_unix_ms,
    }
}

/// Workflow #438 HELD-1 (settled by the coordinator on 11-12; `contracts/scheduling/README.md`,
/// "Held records", "The clock"): the three waits for one fact's refresh.
const REFRESH_WAITS: [PolicyCode; 3] = [
    PolicyCode::FactRefreshPending,
    PolicyCode::FactRefreshUnresolved,
    PolicyCode::FactRefreshExhausted,
];

/// Whether a row whose `why` was `earlier` and is `now` keeps its `since_unix_ms`: the same code,
/// or (HELD-1) two of the three waits for one fact's refresh that name the same `fact_key`, which
/// share one clock. Another `fact_key` of one of them starts another run, as any other code does.
fn same_clock(earlier: &Outcome, now: &Outcome) -> bool {
    let refresh_key = |why: &Outcome| {
        REFRESH_WAITS
            .iter()
            .any(|code| why.is(*code))
            .then(|| why.value("fact_key").cloned())
    };
    match (refresh_key(earlier), refresh_key(now)) {
        (Some(earlier_key), Some(now_key)) => earlier_key == now_key,
        (None, None) => earlier.code() == now.code(),
        _ => false,
    }
}

/// IDLE-WG and #484 IDLE-W2: what holds a due candidate back for the idle plan, if anything does:
/// a time-bound gate of its instance (`held_back`) and a later position a hold of the instance hides
/// (`held_ahead`), each with the time it lets the candidate through (none: no such time). With
/// both, the later of the two, none when either names none.
fn held_until(
    held_back: Option<Option<u64>>,
    held_ahead: Option<Option<u64>>,
) -> Option<Option<u64>> {
    match (held_back, held_ahead) {
        (Some(gates), Some(later)) => {
            Some(gates.zip(later).map(|(gate, release)| gate.max(release)))
        }
        (held, None) | (None, held) => held,
    }
}

fn earliest(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (wake, None) | (None, wake) => wake,
    }
}
