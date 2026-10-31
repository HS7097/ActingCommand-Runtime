// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19a (register §1.143 M19A-2; `contracts/scheduling/README.md`, "Why
//! A Candidate Does Not Run"): the latest evaluation's not-run explanation, per instance, kept in
//! the policy host's memory. Every evaluated cycle replaces it from the bound evaluation, before
//! the host adds its notes to the decisions; a deferred cycle and a failed evaluation leave it as
//! it was, and a start begins with none. Nothing here reaches the ledger and no decision reads
//! it: only the read `ScheduleExplanation` and the idle check (IDLE-1, its plan per instance) do.

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
#[derive(Debug, Default)]
struct IdleDraft {
    task_due: bool,
    task_wake: Option<u64>,
    reading_due: Option<u64>,
}

impl IdleDraft {
    /// `held_back`: the instance's entry of [`IdleHeldBack`], when a time-bound gate holds it.
    /// `update_held`: the instance waits for its client update and the decision is no store update
    /// task's ([`crate::application_update::IdleUpdateHold::holds`]).
    fn add(
        &mut self,
        decision: &TaskDecision,
        evaluated_at_unix_ms: u64,
        held_back: Option<Option<u64>>,
        update_held: bool,
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
        let benefit_held = decision.reasons.last().is_some_and(|reason| {
            reason.outcome.is(PolicyCode::BenefitUnknown)
                || reason.outcome.is(PolicyCode::PolicyBenefitNonpositive)
                || reason.outcome.is(PolicyCode::PolicyNoProgress)
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
            && let Some(through) = held_back
        {
            // IDLE-WG: not dispatchable now; it wakes the instance when the gates let it through.
            self.task_wake = earliest(self.task_wake, through);
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
            self.task_wake = earliest(self.task_wake, decision.next_wake_unix_ms);
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

    /// Replaces the table from an evaluated cycle's bound evaluation (`catalog_hash` its
    /// catalog's, `evaluated_at_unix_ms` its time). A row's `since_unix_ms` carries over from the
    /// table it replaces while the same (instance, task) keeps the same `why` code, and the
    /// not-running row's while the same (instance, task or none) keeps it; otherwise it is this
    /// evaluation's time. SCH-M ED-2 (§3.4 "Visible"): `expected` gives, for a (task, instance
    /// alias), the duration its dispatch would reserve and where it comes from (`declared` or
    /// `runs:<n>`); a row carries both. IDLE-WG (0.12.27): `held_back`, the instances a
    /// time-bound gate holds back as the evaluation ran ([`idle_held_back`]), which only the idle
    /// plan reads; the rows stay the evaluation's own. UPD-1 fix round 1 (review M1):
    /// `update_hold`, the instances position 4a holds as the evaluation ran and the store update
    /// tasks it lets through, which only the idle plan reads too.
    pub(crate) fn record_held_back(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
        expected: &ExpectedRuntimeOf<'_>,
        held_back: &IdleHeldBack,
        update_hold: IdleUpdateHold<'_>,
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
                held_back.get(alias).copied(),
                update_hold.holds(decision),
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
            instances.insert(
                alias.to_owned(),
                InstanceRows {
                    next_wake_unix_ms,
                    selected: selected.remove(alias),
                    candidates,
                    not_running,
                    idle: idle
                        .remove(alias)
                        .unwrap_or_default()
                        .plan(evaluated_at_unix_ms),
                },
            );
        }
        self.latest = Some(Explanation {
            evaluated_at_unix_ms,
            catalog_hash: catalog_hash.to_owned(),
            instances,
        });
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
        .filter(|row| row.why.code() == draft.why.code())
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
        .filter(|row| row.task_id == task_id && row.why.code() == why.code())
        .map_or(evaluated_at_unix_ms, |row| row.since_unix_ms);
    ScheduleExplanationNotRunning {
        task_id,
        why,
        next_wake_unix_ms: row_wake,
        since_unix_ms,
    }
}

fn earliest(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (wake, None) | (None, wake) => wake,
    }
}
