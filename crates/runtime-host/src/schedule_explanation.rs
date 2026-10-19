// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19a (register §1.143 M19A-2; `contracts/scheduling/README.md`, "Why
//! A Candidate Does Not Run"): the latest evaluation's not-run explanation, per instance, kept in
//! the policy host's memory. Every evaluated cycle replaces it from the bound evaluation, before
//! the host adds its notes to the decisions; a deferred cycle and a failed evaluation leave it as
//! it was, and a start begins with none. Nothing here reaches the ledger and no decision reads
//! it: only the read `ScheduleExplanation` does.

use actingcommand_contract::codes::ScheduleExplanationRowCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    MAX_SCHEDULE_EXPLANATION_CANDIDATES, MAX_SCHEDULE_EXPLANATION_REASONS,
    RuntimeScheduleExplanation, ScheduleCandidateEligibility, ScheduleCandidateState,
    ScheduleDispatchState, ScheduleExplanationCandidate, ScheduleExplanationInstance,
    ScheduleExplanationNotRunning, ScheduleExplanationSelected,
};
use actingcommand_policy::{
    EligibilityState, PolicyEvaluation, SchedulingDecisionState, TaskDecision,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;

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
}

/// One unselected decision with its rank, before the rows are ordered.
struct Draft<'a> {
    decision: &'a TaskDecision,
    why: &'a Outcome,
}

impl ScheduleExplanationTable {
    /// Replaces the table from an evaluated cycle's bound evaluation (`catalog_hash` its
    /// catalog's, `evaluated_at_unix_ms` its time). A row's `since_unix_ms` carries over from the
    /// table it replaces while the same (instance, task) keeps the same `why` code, and the
    /// not-running row's while the same (instance, task or none) keeps it; otherwise it is this
    /// evaluation's time.
    pub(crate) fn record(
        &mut self,
        evaluation: &PolicyEvaluation,
        catalog_hash: &str,
        evaluated_at_unix_ms: u64,
    ) {
        let previous = self.latest.take();
        let mut wakes = BTreeMap::<&str, Option<u64>>::new();
        let mut selected = BTreeMap::<&str, (String, String)>::new();
        let mut drafts = BTreeMap::<&str, Vec<Draft<'_>>>::new();
        for decision in &evaluation.decisions {
            let Some(alias) = decision.instance_id.as_deref() else {
                continue;
            };
            let wake = wakes.entry(alias).or_default();
            *wake = earliest(*wake, decision.next_wake_unix_ms);
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
                .map(|draft| candidate_row(draft, earlier, evaluated_at_unix_ms))
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
                },
            );
        }
        self.latest = Some(Explanation {
            evaluated_at_unix_ms,
            catalog_hash: catalog_hash.to_owned(),
            instances,
        });
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
