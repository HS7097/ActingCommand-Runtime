// SPDX-License-Identifier: AGPL-3.0-only

//! UPD-1 (design-upd1 §3, §4; register §1.215; Alice's rulings 95 and 96 of 2026-10-29): the
//! instance fact `health.application.update_required` and the scheduling gate it holds.
//!
//! - **The fact** (`contracts/instance-fact-store.md`, "Application update facts"): an
//!   instance-scoped boolean of the `health.` family that only the Runtime writes
//!   (`application_update_key_reserved` refuses it from a client). True before the terminal of
//!   a run that a shared interrupt page with `result: application_update_required` ended; false
//!   before the successful terminal of a `store_update` package's own run; false as a start's seed
//!   of a configured instance with no record, observed at the earliest valid time
//!   ([`APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS`], fix round 2), never at the start's own
//!   time. It never expires and carries no `invalidate_on`; a
//!   restart, a takeover and an emulator stop leave it as it is. It is an ordinary evaluation
//!   fact, so a store update rule's trigger reads it.
//! - **The gate** (`contracts/scheduling-blackout.md`, "Order of the deferrals", position 4a):
//!   while an instance's fact is true, every candidate of the instance except a `store_update`
//!   package's task is deferred with `application_update_pending` and no wake of its own; the fact
//!   refresh's deferral names it as its gate. The idle plan wakes no instance for such a decision,
//!   nor (fix round 1, review M1) for any other decision of a waiting instance but a store update
//!   task's: a task not a candidate yet (its cooldown, its clock trigger) would only be held by the
//!   gate once it became one ([`IdleUpdateHold`]).
//!
//! The Runtime names no game, store or page here.

use crate::codes::ApplicationUpdateCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{
    CandidateEligibility, DecisionReason, EvaluationFacts, FactValue, ScopeSelector, TaskDecision,
};
use std::collections::BTreeSet;

/// The fact's key: an instance-scoped key of the `health.` family only the Runtime writes.
pub(crate) const APPLICATION_UPDATE_FACT_KEY: &str = "health.application.update_required";
/// The record's `source_detector`.
pub(crate) const APPLICATION_UPDATE_DETECTOR: &str = "runtime.application-update";
/// The record's `schema_version`.
pub(crate) const APPLICATION_UPDATE_SCHEMA: &str = "fact.v1";
/// The `source_snapshot_id` prefix of a run's write (`application_update:run:<run_id>`).
pub(crate) const APPLICATION_UPDATE_RUN_PREFIX: &str = "application_update:run:";
/// The `source_snapshot_id` of a start's seed.
pub(crate) const APPLICATION_UPDATE_SEED: &str = "application_update:seed";
/// UPD-1 fix round 2: a start's seed's `observed_at_unix_ms`, the earliest time a fact record may
/// carry (`FactRecord::validate` refuses 0). The seed states the default, not something the
/// Runtime saw. Dated at the start, the seed post-dated every evaluation or forward projection
/// asked about an earlier time, and each refused its whole input for it (the policy evaluator's
/// `observed_after_evaluation`). Dated here, it never does, and the first record a run writes is
/// always newer than it.
pub(crate) const APPLICATION_UPDATE_SEED_OBSERVED_AT_UNIX_MS: u64 = 1;

/// The instance aliases whose `health.application.update_required` is true in `facts` (with a
/// confidence above zero). Only `true` holds: a missing key or `false` holds nothing.
pub(crate) fn pending_instances(facts: &EvaluationFacts) -> BTreeSet<String> {
    facts
        .facts
        .iter()
        .filter(|fact| {
            fact.fact_key == APPLICATION_UPDATE_FACT_KEY
                && fact.confidence_milli > 0
                && matches!(fact.value, FactValue::Boolean(true))
        })
        .filter_map(|fact| match &fact.scope {
            ScopeSelector::Instance { instance_id } => Some(instance_id.clone()),
            ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => None,
        })
        .collect()
}

/// Position 4a's deferral of a candidate on the instance `instance_id`: no wake of its own (the
/// fact's change wakes the evaluation); a fact refresh's names the gate, as a blackout's does.
pub(crate) fn deferral(instance_id: &str, fact_refresh: bool) -> CandidateEligibility {
    let reason = DecisionReason {
        outcome: Outcome::new(ApplicationUpdateCode::Pending)
            .instance_id(instance_id)
            .fact_key(APPLICATION_UPDATE_FACT_KEY),
        detail: format!(
            "Runtime application update pending: instance_id={instance_id}; fact_key={APPLICATION_UPDATE_FACT_KEY}"
        ),
    };
    if fact_refresh {
        CandidateEligibility::Refresh {
            reason,
            next_wake_unix_ms: None,
            gate: ApplicationUpdateCode::Pending.code(),
            limit: None,
        }
    } else {
        CandidateEligibility::Deferred {
            reason,
            next_wake_unix_ms: None,
        }
    }
}

/// Whether position 4a held `decision`: one of its reasons is `application_update_pending`, or
/// names it as its gate (a fact refresh's `fact_refresh_pending`). The idle plan counts such a
/// decision neither due nor as a wake ([`IdleUpdateHold::holds`] keeps this as its fallback).
pub(crate) fn holds(decision: &TaskDecision) -> bool {
    let code = ApplicationUpdateCode::Pending.as_str();
    decision.reasons.iter().any(|reason| {
        reason.outcome.code().as_str() == code
            || reason
                .outcome
                .value("gate_code")
                .and_then(serde_json::Value::as_str)
                == Some(code)
    })
}

/// UPD-1 fix round 1 (review M1; design §4.2): what the idle plan reads of position 4a, from the
/// evaluation it plans after: the instances whose fact was true ([`pending_instances`]) and the
/// store update tasks (the remedy table's, as the gate read them). The gate only sees candidates,
/// so a task of a waiting instance that is no candidate yet (its cooldown, a clock trigger not
/// come) names its own wake and no position 4a; once it became a candidate the gate would hold it.
/// The idle plan therefore counts no decision of a waiting instance but a store update task's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdleUpdateHold<'a> {
    /// The instances whose `health.application.update_required` was true in the evaluation.
    pub(crate) pending: &'a BTreeSet<String>,
    /// The catalog tasks whose bound package declares `store_update`.
    pub(crate) store_tasks: &'a BTreeSet<String>,
}

/// The empty set the test-only default of `IdleUpdateHold` names.
#[cfg(test)]
static NO_IDS: BTreeSet<String> = BTreeSet::new();

#[cfg(test)]
impl Default for IdleUpdateHold<'_> {
    /// No instance waits.
    fn default() -> Self {
        Self {
            pending: &NO_IDS,
            store_tasks: &NO_IDS,
        }
    }
}

impl IdleUpdateHold<'_> {
    /// Whether the idle plan leaves `decision` out: its instance waits for its client update and
    /// its task is no store update task, whatever its reason; or, as a fallback, position 4a held
    /// it by name ([`holds`]).
    pub(crate) fn holds(&self, decision: &TaskDecision) -> bool {
        let waiting = decision
            .instance_id
            .as_deref()
            .is_some_and(|alias| self.pending.contains(alias));
        (waiting && !self.store_tasks.contains(decision.task_id.as_str())) || holds(decision)
    }
}
