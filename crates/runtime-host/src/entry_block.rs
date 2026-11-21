// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #490 ENTRY-A (`contracts/scheduling-blackout.md`, "Order of the deferrals", position
//! 4b; `contracts/policy-suspension.md`, "A failure at the entry"): the scheduling gate of an
//! instance that a scheduled run's failure at its entry blocked. The host keeps the blocks in
//! memory (`host/entry_blocks.rs`) and hands the evaluation each blocked instance by alias as the
//! cycle's inputs are read; this is what the evaluation answers the instance's candidates: every
//! candidate (an owed rerun, an immediate rerun, a remedy request and the fact refresh included)
//! is deferred with `instance_entry_blocked` and no wake of its own, whatever its rank, but (fix
//! round 1) a store update package's task, which passes as at position 4a: an instance whose game
//! asks for an update fails every entry, and only that task ends it. The block's end (a recovery
//! or a run of the instance that succeeded, a resume, a restart, a covering blackout, a fence)
//! wakes the evaluation or hands the instance to what owns it. Fix round 4 (rulings 2 and 3): a
//! pair (instance, catalog task) whose scheduled runs keep failing at their entry waits after each
//! failure (`instance_entry_task_waiting`, until the wait's end) and, once its failures that came
//! although the instance let another scheduled run in reach its rule's `escalation_threshold`, is
//! set aside until the end of its activity window (`instance_entry_task_set_aside`); both wake at
//! their end, and neither outlives the window. The Runtime names no game, package or page here.

use crate::codes::HostCode;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_policy::{CandidateEligibility, DecisionReason, InstanceHolds};
use std::collections::{BTreeMap, BTreeSet};

/// One blocked instance as the evaluation reads it: the run whose entry failure blocked it and
/// since when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryBlockGate {
    /// The catalog task of the run whose entry failure blocked the instance.
    pub(crate) task_id: String,
    /// That run's dispatch.
    pub(crate) decision_id: String,
    /// Its failure's code.
    pub(crate) failure_code: CodeStr,
    /// When its settlement blocked the instance (its execution record's observation time).
    pub(crate) since_unix_ms: u64,
}

/// Fix round 4 (rulings 2 and 3; `contracts/policy-suspension.md`, "A pair that keeps failing at
/// its entry"): a pair (instance, catalog task) whose latest scheduled runs failed at their entry,
/// as the evaluation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryPairGate {
    /// The pair's latest run that failed at its entry: its dispatch, its failure's code and its
    /// execution record's observation time (its settlement).
    pub(crate) decision_id: String,
    pub(crate) failure_code: CodeStr,
    pub(crate) observed_at_unix_ms: u64,
    /// Its failures at the entry in a row in the window (ruling 2).
    pub(crate) in_a_row: u32,
    /// The pair is not dispatched on the instance before this time (ruling 2).
    pub(crate) until_unix_ms: u64,
    /// The end of the activity window those failures' admissions were committed in (`0` when the
    /// policy knew none: nothing is set aside then).
    pub(crate) window_end_unix_ms: u64,
    /// Ruling 3: set aside until the window's end, with its failures that came although another
    /// scheduled run was let in since the previous one, and its rule's `escalation_threshold`.
    pub(crate) set_aside: Option<(u32, u16)>,
}

/// The blocked instances and (fix round 4) the pairs that wait or are set aside, by alias, as the
/// evaluation reads them.
#[derive(Debug, Clone, Default)]
pub(crate) struct EntryBlockGates {
    blocked: BTreeMap<String, EntryBlockGate>,
    /// Fix round 4: by instance alias and catalog task.
    pairs: BTreeMap<(String, String), EntryPairGate>,
}

impl EntryBlockGates {
    pub(crate) fn new(
        blocked: BTreeMap<String, EntryBlockGate>,
        pairs: BTreeMap<(String, String), EntryPairGate>,
    ) -> Self {
        Self { blocked, pairs }
    }

    /// The gate of the instance `instance_id` (its alias) when it is blocked.
    pub(crate) fn get(&self, instance_id: &str) -> Option<&EntryBlockGate> {
        self.blocked.get(instance_id)
    }

    /// Fix round 4 (rulings 2 and 3; position 4b): the deferral of the task `task_id`'s candidate
    /// on the instance `instance_id` (its alias) at `now_unix_ms`, when its pair is set aside
    /// until its activity window's end (`instance_entry_task_set_aside`) or waits after its latest
    /// failure at the entry (`instance_entry_task_waiting`); each wakes at its end, and neither
    /// holds once the window has ended. `None` for any other pair. A fact refresh's names the
    /// code as its gate.
    pub(crate) fn pair_deferral(
        &self,
        instance_id: &str,
        task_id: &str,
        now_unix_ms: u64,
        fact_refresh: bool,
    ) -> Option<CandidateEligibility> {
        let pair = self
            .pairs
            .get(&(instance_id.to_owned(), task_id.to_owned()))?;
        let window_end = pair.window_end_unix_ms;
        if window_end > 0 && now_unix_ms >= window_end {
            return None;
        }
        let pair_outcome = |code: HostCode| {
            let outcome = Outcome::new(code)
                .instance_id(instance_id)
                .task_id(task_id)
                .decision_id(pair.decision_id.as_str())
                .observed_at_unix_ms(pair.observed_at_unix_ms);
            match pair.failure_code.registered() {
                Some(failure_code) => outcome.failure_code(failure_code),
                None => outcome,
            }
        };
        let (code, outcome, wake) = match pair.set_aside {
            Some((count, threshold)) if window_end > 0 => {
                let code = HostCode::InstanceEntryTaskSetAside;
                let outcome = pair_outcome(code)
                    .count(count)
                    .limit_count(threshold)
                    .window_end_unix_ms(window_end);
                (code, outcome, window_end)
            }
            _ => {
                let until = if window_end > 0 {
                    pair.until_unix_ms.min(window_end)
                } else {
                    pair.until_unix_ms
                };
                if now_unix_ms >= until {
                    return None;
                }
                let code = HostCode::InstanceEntryTaskWaiting;
                let outcome = pair_outcome(code)
                    .count(pair.in_a_row)
                    .next_eligible_unix_ms(until);
                (code, outcome, until)
            }
        };
        let reason = DecisionReason {
            outcome,
            detail: format!("Runtime instance entry task held: instance_id={instance_id}"),
        };
        Some(if fact_refresh {
            CandidateEligibility::Refresh {
                reason,
                next_wake_unix_ms: Some(wake),
                gate: code.code(),
                limit: None,
            }
        } else {
            CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms: Some(wake),
            }
        })
    }
}

/// `instance_entry_blocked` of the instance `instance_id` (its alias) that `gate` blocked: the
/// run whose failure at its entry set it, its failure's code when the catalog registers it, and
/// since when.
fn outcome(instance_id: &str, gate: &EntryBlockGate) -> Outcome {
    let outcome = Outcome::new(HostCode::InstanceEntryBlocked)
        .instance_id(instance_id)
        .task_id(gate.task_id.as_str())
        .decision_id(gate.decision_id.as_str())
        .observed_at_unix_ms(gate.since_unix_ms);
    match gate.failure_code.registered() {
        Some(code) => outcome.failure_code(code),
        None => outcome,
    }
}

/// Position 4b's reason for a candidate on the instance `instance_id`.
fn reason(instance_id: &str, gate: &EntryBlockGate) -> DecisionReason {
    DecisionReason {
        outcome: outcome(instance_id, gate),
        detail: format!("Runtime instance entry blocked: instance_id={instance_id}"),
    }
}

/// Position 4b as the evaluator's whole-instance holds (as position 4a's, `application_update`):
/// each blocked instance, every task held but (fix round 1) a store update package's task
/// (`store_tasks`), so a candidate the round settles before it asks the eligibility closure waits
/// with the block's reason too. An instance position 4a holds (`update_pending`) keeps 4a's hold,
/// the earlier position, which lets the same tasks pass.
pub(crate) fn hold(
    holds: &mut InstanceHolds,
    gates: &EntryBlockGates,
    update_pending: &BTreeSet<String>,
    store_tasks: &BTreeSet<String>,
) {
    for (instance_id, gate) in &gates.blocked {
        if !update_pending.contains(instance_id) {
            holds.hold(
                instance_id.clone(),
                reason(instance_id, gate),
                HostCode::InstanceEntryBlocked.code(),
                store_tasks.clone(),
            );
        }
    }
}

/// Position 4b's deferral of a candidate on the instance `instance_id`: no wake of its own (the
/// block's end wakes the evaluation); a fact refresh's names the block as its gate.
pub(crate) fn deferral(
    instance_id: &str,
    gate: &EntryBlockGate,
    fact_refresh: bool,
) -> CandidateEligibility {
    let reason = reason(instance_id, gate);
    if fact_refresh {
        CandidateEligibility::Refresh {
            reason,
            next_wake_unix_ms: None,
            gate: HostCode::InstanceEntryBlocked.code(),
            limit: None,
        }
    } else {
        CandidateEligibility::Deferred {
            reason,
            next_wake_unix_ms: None,
        }
    }
}
