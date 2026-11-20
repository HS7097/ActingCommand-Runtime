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
//! wakes the evaluation or hands the instance to what owns it. The Runtime names no game, package
//! or page here.

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

/// The blocked instances and (fix round 3, ruling 2) the pairs that yield their turn, by alias,
/// as the evaluation reads them.
#[derive(Debug, Clone, Default)]
pub(crate) struct EntryBlockGates {
    blocked: BTreeMap<String, EntryBlockGate>,
    /// Fix round 3: each instance whose block ended, with the gate of that block: the pair whose
    /// failure at the entry set it (`task_id`) yields its turn there.
    yielding: BTreeMap<String, EntryBlockGate>,
}

impl EntryBlockGates {
    pub(crate) fn new(
        blocked: BTreeMap<String, EntryBlockGate>,
        yielding: BTreeMap<String, EntryBlockGate>,
    ) -> Self {
        Self { blocked, yielding }
    }

    /// The gate of the instance `instance_id` (its alias) when it is blocked.
    pub(crate) fn get(&self, instance_id: &str) -> Option<&EntryBlockGate> {
        self.blocked.get(instance_id)
    }
}

/// `code` of the instance `instance_id` (its alias) and the block `gate`: the run whose failure at
/// its entry set it, its failure's code when the catalog registers it, and since when.
fn outcome_of(code: HostCode, instance_id: &str, gate: &EntryBlockGate) -> Outcome {
    let outcome = Outcome::new(code)
        .instance_id(instance_id)
        .task_id(gate.task_id.as_str())
        .decision_id(gate.decision_id.as_str())
        .observed_at_unix_ms(gate.since_unix_ms);
    match gate.failure_code.registered() {
        Some(code) => outcome.failure_code(code),
        None => outcome,
    }
}

/// `instance_entry_blocked` of the instance `instance_id` (its alias) that `gate` blocked.
fn outcome(instance_id: &str, gate: &EntryBlockGate) -> Outcome {
    outcome_of(HostCode::InstanceEntryBlocked, instance_id, gate)
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
/// the earlier position, which lets the same tasks pass. Fix round 3 (ruling 2;
/// `contracts/policy-suspension.md`, "The pair yields its turn"): on an instance whose block
/// ended, the pair whose failure at the entry set that block yields its turn
/// (`InstanceHolds::yield_turn`) until another run started there; when a candidate it yielded to
/// takes the instance it waits `instance_entry_yielding`, a fact refresh naming that code.
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
    for (instance_id, gate) in &gates.yielding {
        holds.yield_turn(
            instance_id.clone(),
            gate.task_id.clone(),
            DecisionReason {
                outcome: outcome_of(HostCode::InstanceEntryYielding, instance_id, gate),
                detail: format!("Runtime instance entry yielding: instance_id={instance_id}"),
            },
            HostCode::InstanceEntryYielding.code(),
        );
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
