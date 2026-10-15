// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S5 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §5.1 MP-3, §5.3 A-5,
//! §6 S5, §7): the fact refresh's bounded transition recorder.
//!
//! At each policy cycle's yield point the recorder reads the reasons the evaluation gave the
//! candidates that wait for a key (`fact_refresh_pending`, `fact_refresh_unresolved`,
//! `fact_refresh_exhausted`, `fact_refresh_unbound`, D2 model §3.7) and turns their changes into
//! transitions: a key starts to be waited for, the reason's code changes, the key is no longer
//! waited for (`fact_refresh_resolved`), or one wait has lasted `overdue_ms`
//! (`fact_reader_overdue`, once per wait). It only produces transitions: no decision reads it,
//! and it writes nothing to the ledger (the ledger observer's G5 gate copies the transitions
//! later as `policy.fact_refresh_transition`, register D1).
//!
//! - **Bounded.** It holds one entry per (instance, key) the latest observed cycle found waited
//!   for, so at most the instances times the keys their candidates read; an entry leaves with
//!   its `fact_refresh_resolved`.
//! - **Numbered.** Cycles and transitions are numbered from 1 per owner epoch (one recorder per
//!   Runtime start), one by one; the transition number is never reset (A-5).
//! - **An instance held at the cycle's start** (the closure's positions 1-2: an operator pause, a
//!   holder, a waiting claim, a takeover cooldown) raises no demand that cycle (D2 model §3.4
//!   step 1): its keys are not observed, so they keep their state and their wait goes on.

use crate::codes::HostCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    RuntimeFactRefreshDemand, RuntimeFactRefreshInstance, RuntimeFactRefreshStatus,
};
use actingcommand_policy::{FACT_REFRESH_TASK_ID, PolicyCode, PolicyEvaluation};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One transition of the fact refresh recorder: the cycle it happened in, its number in this
/// owner epoch, the instance and key, and its code with its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactRefreshTransition {
    pub cycle_seq: u64,
    pub transition_seq: u64,
    pub instance_id: String,
    pub fact_key: String,
    pub outcome: Outcome,
}

/// One (instance, key) the candidates wait for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    /// The latest observed cycle's reason.
    outcome: Outcome,
    /// The instance's recognition maintenance package, when one serves the key.
    package_id: Option<String>,
    /// When the wait began: the cycle that first found the key waited for.
    since_unix_ms: u64,
    /// `fact_reader_overdue` was reported for this wait.
    overdue: bool,
}

/// The recorder of one owner epoch (see the module).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FactRefreshRecorder {
    cycle_seq: u64,
    transition_seq: u64,
    waiting: BTreeMap<(String, String), Waiting>,
}

/// Whether `outcome` is a reason a waiting candidate carries for one key (D2 model §3.7 rows
/// 2-6).
fn waits(outcome: &Outcome) -> bool {
    outcome.is(PolicyCode::FactRefreshPending)
        || outcome.is(PolicyCode::FactRefreshUnresolved)
        || outcome.is(PolicyCode::FactRefreshExhausted)
        || outcome.is(PolicyCode::FactRefreshUnbound)
}

fn text<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

/// The keys the candidates of `evaluation` wait for, by (instance, key): the first such reason in
/// decision order. Every one of these reasons names its instance and key (D2 model §7.1); the
/// recognition maintenance package's own decision is no candidate's.
fn waited_for(evaluation: &PolicyEvaluation) -> BTreeMap<(String, String), &Outcome> {
    let mut waited = BTreeMap::new();
    for decision in evaluation
        .decisions
        .iter()
        .filter(|decision| decision.task_id != FACT_REFRESH_TASK_ID)
    {
        for reason in decision
            .reasons
            .iter()
            .filter(|reason| waits(&reason.outcome))
        {
            let (Some(instance), Some(key)) = (
                text(&reason.outcome, "instance_id"),
                text(&reason.outcome, "fact_key"),
            ) else {
                continue;
            };
            waited
                .entry((instance.to_owned(), key.to_owned()))
                .or_insert(&reason.outcome);
        }
    }
    waited
}

impl FactRefreshRecorder {
    /// Observes one evaluated cycle at `now_unix_ms` and returns its transitions, in (instance,
    /// key) order: for each key, its state change first, then its `fact_reader_overdue` when
    /// `overdue_ms` is set and its wait reached it now. `held` tells an instance the closure's
    /// positions 1-2 held as the cycle started. Nothing here is read by any decision.
    pub(crate) fn observe(
        &mut self,
        evaluation: &PolicyEvaluation,
        held: &dyn Fn(&str) -> bool,
        now_unix_ms: u64,
        overdue_ms: Option<u64>,
    ) -> Vec<FactRefreshTransition> {
        self.cycle_seq = self.cycle_seq.saturating_add(1);
        let current = waited_for(evaluation);
        let keys = self
            .waiting
            .keys()
            .chain(current.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut transitions = Vec::new();
        for key in keys {
            let (instance, fact_key) = &key;
            match (self.waiting.contains_key(&key), current.get(&key).copied()) {
                (false, Some(outcome)) => {
                    self.waiting.insert(
                        key.clone(),
                        Waiting {
                            outcome: outcome.clone(),
                            package_id: text(outcome, "package_id").map(str::to_owned),
                            since_unix_ms: now_unix_ms,
                            overdue: false,
                        },
                    );
                    self.push(&mut transitions, instance, fact_key, outcome.clone());
                }
                (true, Some(outcome)) => {
                    // The latest reason with its values (a gate, a wake), whether or not its code
                    // changed: the status read shows it.
                    let changed = self.waiting.get_mut(&key).is_some_and(|waiting| {
                        let changed = waiting.outcome.code() != outcome.code();
                        waiting.outcome = outcome.clone();
                        waiting.package_id = text(outcome, "package_id").map(str::to_owned);
                        changed
                    });
                    if changed {
                        self.push(&mut transitions, instance, fact_key, outcome.clone());
                    }
                }
                (true, None) if !held(instance) => {
                    if let Some(waiting) = self.waiting.remove(&key) {
                        let resolved = Outcome::new(PolicyCode::FactRefreshResolved)
                            .instance_id(instance.as_str())
                            .fact_key(fact_key.as_str());
                        let resolved = match waiting.package_id {
                            Some(package_id) => resolved.package_id(package_id.as_str()),
                            None => resolved,
                        };
                        self.push(&mut transitions, instance, fact_key, resolved);
                    }
                    continue;
                }
                // A held instance's keys are not observed this cycle.
                (_, None) => {}
            }
            let overdue = self.waiting.get_mut(&key).and_then(|waiting| {
                let limit = overdue_ms?;
                let elapsed_ms = now_unix_ms.saturating_sub(waiting.since_unix_ms);
                if waiting.overdue
                    || waiting.outcome.is(PolicyCode::FactRefreshUnbound)
                    || elapsed_ms < limit
                {
                    return None;
                }
                waiting.overdue = true;
                let outcome = Outcome::new(HostCode::FactReaderOverdue)
                    .instance_id(instance.as_str())
                    .fact_key(fact_key.as_str())
                    .elapsed_ms(elapsed_ms);
                Some(match &waiting.package_id {
                    Some(package_id) => outcome.package_id(package_id.as_str()),
                    None => outcome,
                })
            });
            if let Some(overdue) = overdue {
                self.push(&mut transitions, instance, fact_key, overdue);
            }
        }
        transitions
    }

    fn push(
        &mut self,
        transitions: &mut Vec<FactRefreshTransition>,
        instance: &str,
        fact_key: &str,
        outcome: Outcome,
    ) {
        self.transition_seq = self.transition_seq.saturating_add(1);
        transitions.push(FactRefreshTransition {
            cycle_seq: self.cycle_seq,
            transition_seq: self.transition_seq,
            instance_id: instance.to_owned(),
            fact_key: fact_key.to_owned(),
            outcome,
        });
    }

    /// What `FactRefreshStatus` answers (register §1.68): the numbers of the latest cycle and
    /// transition and every waited-for key, by instance and key.
    pub(crate) fn status(&self) -> RuntimeFactRefreshStatus {
        let mut instances = Vec::<RuntimeFactRefreshInstance>::new();
        for ((instance, fact_key), waiting) in &self.waiting {
            let demand = RuntimeFactRefreshDemand {
                fact_key: fact_key.clone(),
                outcome: waiting.outcome.clone(),
                package_id: waiting.package_id.clone(),
                since_unix_ms: waiting.since_unix_ms,
                overdue: waiting.overdue,
            };
            match instances.last_mut() {
                Some(row) if row.instance_alias == *instance => row.demands.push(demand),
                _ => instances.push(RuntimeFactRefreshInstance {
                    instance_alias: instance.clone(),
                    demands: vec![demand],
                }),
            }
        }
        RuntimeFactRefreshStatus {
            cycle_seq: self.cycle_seq,
            transition_seq: self.transition_seq,
            instances,
        }
    }
}
