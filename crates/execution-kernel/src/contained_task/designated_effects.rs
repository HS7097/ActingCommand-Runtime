// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #390 OUT-2 (forced compromises O-01, O-02, R-03, S-01): the caps of a task's
//! designated effect set (`contracts/resource-declarations.md`, section Task schema `0.9`
//! designated effect sets). A member's cap is its constant, 1 when it declares none, or the
//! integer value of the instance fact it names. The fact caps are read once, before the run's
//! first step, from the instance's ledger-pinned fact snapshot through the runtime's
//! `selection_state` seam; a cap that cannot be read ends the run before any input with
//! `contained_task_designated_effect_cap_unresolved`.
//!
//! Workflow #477 LR-1 (`contracts/linear-steps.md`, "Pressing on one page"): a linear plan's press
//! counts read from a dispatch parameter are read by the same snapshot and the same rules
//! ([`PreparedContainedTask::cap_facts`], [`CapFacts::read`]), one reading for both.

use super::{
    ContainedTaskError, ContainedTaskRunError, ContainedTaskRuntime, PreparedContainedTask,
    SelectionState, SelectionStateRequest,
};
use crate::DesignatedEffectCap;
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Outcome, VocabularyToken};
use actingcommand_contract::{DesignatedEffectRepeats, MAX_DESIGNATED_EFFECT_REPEATS};
use actingcommand_selection_policy::{
    ScalarValue, SelectionFactEntry, SelectionFactSnapshot, UnknownReason,
};

/// Why a fact cap could not be read, as a token of the catalog vocabulary `unknown_reason`.
#[derive(Clone, Copy)]
struct CapUnknownReason(UnknownReason);

impl VocabularyToken for CapUnknownReason {
    const VOCABULARY: &'static str = "unknown_reason";

    fn token(self) -> &'static str {
        match self.0 {
            UnknownReason::FactMissing => "fact_missing",
            UnknownReason::FactExpired => "fact_expired",
            UnknownReason::FactStale => "fact_stale",
            UnknownReason::FactLowConfidence => "fact_low_confidence",
            UnknownReason::FactNotScalar => "fact_not_scalar",
            UnknownReason::FieldMissing => "field_missing",
            // The vocabulary token shares the registered code's spelling.
            UnknownReason::TypeMismatch => SharedCode::TypeMismatch.as_str(),
            UnknownReason::LookupMiss => "lookup_miss",
        }
    }
}

/// What a fact cap read gave: a cap, or the values of its refusal.
enum FactCap {
    Cap(u32),
    Unknown(UnknownReason),
    OutOfRange(i64),
}

/// The run's fact snapshot, read once for every fact cap of the set. Workflow #477 LR-1: and for
/// every press count of a linear plan that a dispatch parameter gives.
pub(super) struct CapFacts {
    facts: SelectionFactSnapshot,
    now_unix_ms: u64,
}

impl CapFacts {
    fn cap(&self, fact_key: &str) -> FactCap {
        let Some(entry) = self.facts.facts.get(fact_key) else {
            return FactCap::Unknown(UnknownReason::FactMissing);
        };
        match entry {
            SelectionFactEntry::Unusable { reason } => FactCap::Unknown(*reason),
            SelectionFactEntry::RecordList { .. } => FactCap::Unknown(UnknownReason::FactNotScalar),
            SelectionFactEntry::Published {
                value,
                expires_at_unix_ms,
                ..
            } => {
                // The expiry millisecond is still valid (the fact TTL convention).
                if expires_at_unix_ms.is_some_and(|expiry| self.now_unix_ms > expiry) {
                    return FactCap::Unknown(UnknownReason::FactExpired);
                }
                let ScalarValue::Integer(value) = value else {
                    return FactCap::Unknown(UnknownReason::TypeMismatch);
                };
                match u32::try_from(*value) {
                    Ok(cap) if cap <= MAX_DESIGNATED_EFFECT_REPEATS => FactCap::Cap(cap),
                    _ => FactCap::OutOfRange(*value),
                }
            }
        }
    }

    /// The cap the operation `operation_id` reads under `fact_key`: an unexpired integer from 0 to
    /// [`MAX_DESIGNATED_EFFECT_REPEATS`], or `contained_task_designated_effect_cap_unresolved` with
    /// why it is none (`unknown_reason`, or `count` and `limit_count`).
    pub(super) fn read(
        &self,
        operation_id: &str,
        fact_key: &str,
    ) -> Result<u32, ContainedTaskError> {
        match self.cap(fact_key) {
            FactCap::Cap(cap) => Ok(cap),
            FactCap::Unknown(reason) => Err(unresolved(operation_id, fact_key, |outcome| {
                outcome.unknown_reason(CapUnknownReason(reason))
            })),
            FactCap::OutOfRange(value) => Err(unresolved(operation_id, fact_key, |outcome| {
                outcome
                    .count(value)
                    .limit_count(MAX_DESIGNATED_EFFECT_REPEATS)
            })),
        }
    }
}

/// `contained_task_designated_effect_cap_unresolved` for the fact cap the operation
/// `operation_id` reads under `fact_key`, with the values `outcome` adds.
fn unresolved(
    operation_id: &str,
    fact_key: &str,
    outcome: impl FnOnce(Outcome) -> Outcome,
) -> ContainedTaskError {
    ContainedTaskError::valued(
        ExecutionKernelCode::ContainedTaskDesignatedEffectCapUnresolved.code(),
        |base| outcome(base.pack_operation_id(operation_id).fact_key(fact_key)),
    )
}

impl PreparedContainedTask {
    /// The caps a run holds its designated effect set to, in declaration order; empty for a task
    /// without a set. The fact caps are read here, before the run's first step.
    pub(super) fn designated_effect_caps<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Result<Vec<DesignatedEffectCap>, ContainedTaskRunError<R::Error>> {
        let Some(effects) = self
            .program
            .scheduling_outcome
            .as_ref()
            .and_then(|declaration| declaration.designated_effects())
        else {
            return Ok(Vec::new());
        };
        let mut facts: Option<CapFacts> = None;
        let mut caps = Vec::with_capacity(effects.len());
        for effect in effects {
            let max_repeats = match effect.max_repeats() {
                None => 1,
                Some(DesignatedEffectRepeats::Constant(cap)) => *cap,
                Some(DesignatedEffectRepeats::Fact(fact)) => {
                    let fact_key = fact.fact_key();
                    let read = match facts.take() {
                        Some(read) => read,
                        None => self.cap_facts(runtime, effect.operation_id(), fact_key)?,
                    };
                    let cap = read.read(effect.operation_id(), fact_key);
                    facts = Some(read);
                    cap?
                }
            };
            caps.push(DesignatedEffectCap {
                operation_id: effect.operation_id().to_owned(),
                max_repeats,
            });
        }
        Ok(caps)
    }

    /// The instance's fact snapshot for the fact caps, asked for the operation `operation_id`
    /// whose cap reads `fact_key`. A runtime without one, or a snapshot that is invalid or of
    /// another game or server, leaves that cap unresolved.
    pub(super) fn cap_facts<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation_id: &str,
        fact_key: &str,
    ) -> Result<CapFacts, ContainedTaskRunError<R::Error>> {
        let state = runtime
            .selection_state(SelectionStateRequest {
                step_index: 0,
                operation_label: operation_id.to_owned(),
                game: self.control.game.clone(),
                server: self.control.server.clone(),
            })
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let SelectionState::Snapshot {
            snapshot,
            now_unix_ms,
        } = state
        else {
            return Err(unresolved(operation_id, fact_key, |outcome| outcome).into());
        };
        if now_unix_ms == 0
            || snapshot.validate().is_err()
            || snapshot.context.game_id != self.control.game
            || snapshot.context.server_id != self.control.server
        {
            return Err(unresolved(operation_id, fact_key, |outcome| outcome).into());
        }
        Ok(CapFacts {
            facts: SelectionFactSnapshot::from_instance_snapshot(&snapshot, now_unix_ms),
            now_unix_ms,
        })
    }
}
