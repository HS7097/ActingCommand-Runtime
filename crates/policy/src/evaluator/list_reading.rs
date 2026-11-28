// SPDX-License-Identifier: AGPL-3.0-only

//! #455 RR3e ruling 2 (register §1.560; `contracts/scheduling/README.md`, "A pool read from rows
//! of a list", "The list's reader"): the reading a run left owed to the task that reads a list.
//!
//! - **The binding.** The host gives, beside the readings of the pools read from rows of a list,
//!   the catalog task whose scheduled run published each list's record, as it noted it in memory
//!   when that run published it ([`GivenPoolReadings::reader`]); a list another run published has
//!   none.
//! - **Who is held.** On an instance, a candidate waits on a list when its undecided trigger or
//!   feedback stop suggests the list's key, when its net benefit is unknown on a pool read from
//!   the list, or when it is held as not positive while a goal that covers it waits on such a pool
//!   (#455 RR3e ruling 1): the readings the benefit gate's held candidates wait for (`waits`) and
//!   the suggestions of the undecided ones.
//! - **What is owed.** Where such a list has a reader and a pool read from it is unread, the
//!   production or the consumption of runs having left its reading stale (`fact_stale`), the
//!   reader's pair on the instance is owed the reading once in the pool's unread episode: while
//!   no dispatch of that pair was admitted after the latest of the dispatches that leave the pool
//!   unread ([`HandedRuns::unread_since`]). It is treated as triggered (`list_reading_owed`) and
//!   passes no gate: not promoted, not past its cooldown; its limits, its activity window and
//!   budget, placement, pauses, one winner per instance and the Runtime's own gates apply. On an
//!   instance the idle action closed, a reading alone never wakes the game: it is owed only while
//!   a candidate that waits on the list is due (its trigger true and its feedback stop false), as
//!   the fact refresh's reader is held back there (IDLE-1P).
//!
//! Memory only: nothing is stored and nothing enters a snapshot or a decision identity.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;

use super::{
    DecisionReason, EligibilityState, EvaluationFacts, GivenPoolReadings, HandedRuns, IdleClosed,
    PoolValues, TaskWork, pool_record_times, reason_with,
};
use crate::codes::PolicyCode;
use crate::{ObservationRef, PoolSpec, ScopeSelector, TaskSpec};

/// The readings owed in one round, by (task id, instance alias), each with the reason the
/// reader's decision carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct OwedListReadings {
    pairs: BTreeMap<(String, String), DecisionReason>,
}

impl OwedListReadings {
    /// The reason of the reading owed to the pair, if one is.
    pub(super) fn get(&self, task_id: &str, instance_id: &str) -> Option<&DecisionReason> {
        if self.pairs.is_empty() {
            return None;
        }
        self.pairs
            .get(&(task_id.to_owned(), instance_id.to_owned()))
    }

    /// Whether no reading is owed.
    pub(super) fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// What [`owed`] reads of a round's first judgement.
pub(super) struct Probe<'a> {
    /// The catalog's tasks, as the handed runs are judged against them.
    pub(super) tasks: &'a [TaskSpec],
    pub(super) pool_specs: &'a BTreeMap<&'a str, &'a PoolSpec>,
    /// The evaluation's pool values: which pools are unread, and the readings as the resets leave
    /// them.
    pub(super) pool_values: &'a PoolValues<'a>,
    pub(super) facts: &'a EvaluationFacts,
    /// The readings as the host gave them, with each list's reader.
    pub(super) bindings: &'a GivenPoolReadings,
    pub(super) handed: &'a HandedRuns,
    /// IDLE-1P (#405): the instances the idle action closed as the cycle starts.
    pub(super) idle_closed: Option<&'a BTreeMap<String, IdleClosed>>,
}

/// The readings owed in a round whose first judgement left `work` (each entry's suggestions are
/// its undecided trigger's or feedback stop's) and whose benefit gate's held candidates waited for
/// `waits` (module doc).
pub(super) fn owed(
    probe: &Probe<'_>,
    work: &[TaskWork],
    waits: &BTreeSet<(String, String)>,
) -> OwedListReadings {
    // By instance and key, whether a candidate that waits for it is due. A candidate the benefit
    // gate judged passed its trigger and its feedback stop.
    let mut held = waits
        .iter()
        .map(|wait| (wait.clone(), true))
        .collect::<BTreeMap<_, _>>();
    for entry in work {
        let Some(instance_id) = entry.instance_id.as_deref() else {
            continue;
        };
        let due = entry.eligibility == EligibilityState::True;
        for suggestion in &entry.suggestions {
            if matches!(&suggestion.scope, ScopeSelector::Instance { instance_id: scoped }
                if scoped == instance_id)
            {
                let waiting = held
                    .entry((instance_id.to_owned(), suggestion.fact_key.clone()))
                    .or_insert(false);
                *waiting |= due;
            }
        }
    }
    let mut owed = OwedListReadings::default();
    for ((instance_id, fact_key), due) in held {
        let Some(reader) = probe.bindings.reader(&instance_id, &fact_key) else {
            continue;
        };
        if !due
            && probe
                .idle_closed
                .is_some_and(|closed| closed.contains_key(&instance_id))
        {
            continue;
        }
        let latest = probe.handed.latest_noted(reader, &instance_id);
        // The first pool read from the list on the instance, by id, that the runs left unread and
        // whose unread episode began after the reader's latest dispatch.
        let pool = probe.pool_specs.values().copied().find(|pool| {
            listed_on(pool, &instance_id, &fact_key)
                && probe.pool_values.unread.contains(pool.id.as_str())
                && pool_record_times(pool, probe.facts, probe.pool_values.given())
                    .and_then(|(observed_at_unix_ms, _)| {
                        probe
                            .handed
                            .unread_since(probe.tasks, &pool.id, observed_at_unix_ms)
                    })
                    .is_some_and(|since| latest.is_none_or(|own| own <= since))
        });
        if let Some(pool) = pool {
            let reason = owed_reason(&fact_key, &pool.id);
            owed.pairs.insert((reader.to_owned(), instance_id), reason);
        }
    }
    owed
}

/// Whether `pool` is read from rows of the list `fact_key` of the instance `instance_id`.
fn listed_on(pool: &PoolSpec, instance_id: &str, fact_key: &str) -> bool {
    matches!(&pool.observation, ObservationRef::ListRows { fact_key: listed, .. }
        if listed == fact_key)
        && matches!(&pool.scope, ScopeSelector::Instance { instance_id: scoped }
            if scoped == instance_id)
}

/// `list_reading_owed` (info; `fact_key` the list's key, `pool_id` the pool the runs left unread).
fn owed_reason(fact_key: &str, pool_id: &str) -> DecisionReason {
    reason_with(
        Outcome::new(PolicyCode::ListReadingOwed)
            .fact_key(fact_key)
            .pool_id(pool_id),
        format!(
            "fact_key={fact_key} pool_id={pool_id}; the task that reads the list is treated as \
             triggered once in the pool's unread episode"
        ),
    )
}
