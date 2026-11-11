// SPDX-License-Identifier: AGPL-3.0-only

//! SCHED-C1d (register §1.299 SC1-11; `contracts/scheduling/README.md`, "Episodes survive"): what
//! the Runtime keeps of the capped episodes across a start, and the rules of their restore. The
//! host keeps them as the tagged program fact `host.capped_episodes` (iron rule 13: no state file)
//! and owns its rows; this module owns the shape of an episode and what a start keeps of it:
//!
//! - **Kept** ([`CappedMemory::kept`]): per instance and pool with an open or waiting episode, its
//!   number, its rounds, its consumers key, and while a round is under way the round's start, its
//!   starting reading and its frozen order with the members it passed (and their gate codes).
//!   What the latest evaluation offered to the allocation is no part of it: the next evaluation
//!   offers again.
//! - **Restored** ([`CappedMemory::restore`]): an episode that breaks the shape's rules (a pair
//!   twice, a number of 0, no consumer, a consumption of 0, a task twice, a round without a round
//!   number or without members, a member that is no consumer, a gate code on a member not passed)
//!   fails the whole value; an episode whose pool the active catalog does not give a declared
//!   valuation is dropped alone. A restored episode's number counts on: the next episode of its
//!   pair is the next number. Its consumers are checked against the catalog at the first
//!   evaluation, as at every new generation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use actingcommand_contract::outcome::Code;

use super::{CappedMemory, ConsumerKey, Episode, Member, Round, record_integer};
use crate::CompiledCatalog;

/// SCHED-C1d: one kept episode of a pool on an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptEpisode {
    /// The instance alias.
    pub instance_id: String,
    pub pool_id: String,
    /// Its number since its pair's first episode (`capped_episode`).
    pub number: u64,
    /// How many rounds it began (`capped_round` of the current or last one).
    pub rounds: u64,
    /// The consumers key, by task id.
    pub consumers: Vec<KeptConsumer>,
    /// The round under way; `None` while the episode waits for its next round.
    pub round: Option<KeptRound>,
}

/// SCHED-C1d: one consumer of a kept episode's consumers key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptConsumer {
    pub task_id: String,
    /// `⌈C/1000⌉` of one run.
    pub consumed: u64,
    /// Whether the task is marked `benefit_limited`.
    pub limited: bool,
}

/// SCHED-C1d: the round under way of a kept episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptRound {
    /// When the round began; a task dispatched at or after it counts as dispatched in it.
    pub started_at_unix_ms: u64,
    /// The reading the round's plan starts from.
    pub reading: i64,
    /// The frozen order, the higher `V ÷ c` first.
    pub order: Vec<KeptMember>,
}

/// SCHED-C1d: one member of a kept round's frozen order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptMember {
    pub task_id: String,
    /// `⌈C/1000⌉` its plan consumes.
    pub consumed: u64,
    /// Whether the round passed it (SS-2h fix 1 (b)).
    pub passed: bool,
    /// The registered code of the gate that refused it, when it was passed with one.
    pub gate_code: Option<Code>,
}

/// SCHED-C1d: what a start keeps of the kept episodes ([`CappedMemory::restore`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedRestore {
    pub memory: CappedMemory,
    /// The (instance alias, pool id) of each episode dropped because the active catalog does not
    /// give its pool a declared valuation, in order.
    pub dropped: Vec<(String, String)>,
}

/// SCHED-C1d: why kept episodes break the shape's rules ([`CappedMemory::restore`]); the whole
/// value is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CappedStateProblem {
    /// Two episodes of one (instance alias, pool id).
    Repeated {
        instance_id: String,
        pool_id: String,
    },
    /// One episode breaks a rule of its shape.
    Invalid {
        instance_id: String,
        pool_id: String,
    },
}

impl fmt::Display for CappedStateProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repeated {
                instance_id,
                pool_id,
            } => write!(
                formatter,
                "the kept capped episodes name pool '{pool_id}' on instance '{instance_id}' twice"
            ),
            Self::Invalid {
                instance_id,
                pool_id,
            } => write!(
                formatter,
                "the kept capped episode of pool '{pool_id}' on instance '{instance_id}' breaks \
                 the rules of its shape"
            ),
        }
    }
}

impl CappedMemory {
    /// SCHED-C1d: the episodes as the Runtime keeps them across a start, by (instance alias,
    /// pool id). Empty while no pool is in an episode.
    pub fn kept(&self) -> Vec<KeptEpisode> {
        self.episodes
            .iter()
            .map(|((instance_id, pool_id), episode)| KeptEpisode {
                instance_id: instance_id.clone(),
                pool_id: pool_id.clone(),
                number: episode.number,
                rounds: episode.rounds,
                consumers: episode
                    .consumers
                    .iter()
                    .map(|consumer| KeptConsumer {
                        task_id: consumer.task_id.clone(),
                        consumed: consumer.consumed,
                        limited: consumer.limited,
                    })
                    .collect(),
                round: episode.round.as_ref().map(|round| KeptRound {
                    started_at_unix_ms: round.started_at_unix_ms,
                    reading: record_integer(round.reading),
                    order: round
                        .order
                        .iter()
                        .enumerate()
                        .map(|(index, member)| KeptMember {
                            task_id: member.task_id.clone(),
                            consumed: member.consumed,
                            passed: round.passed.contains_key(&index),
                            gate_code: round.passed.get(&index).copied().flatten(),
                        })
                        .collect(),
                }),
            })
            .collect()
    }

    /// SCHED-C1d: the memory a start builds from `kept` under the active `catalog` (module doc):
    /// every episode whose pool the catalog gives a declared valuation, in the round and with the
    /// members passed it was kept with; the others are named in `dropped`. Kept episodes that
    /// break the shape's rules are refused whole.
    pub fn restore(
        kept: Vec<KeptEpisode>,
        catalog: Option<&CompiledCatalog>,
    ) -> Result<CappedRestore, CappedStateProblem> {
        let mut memory = Self::default();
        let mut dropped = Vec::new();
        let mut seen = BTreeSet::new();
        for episode in kept {
            let key = (episode.instance_id.clone(), episode.pool_id.clone());
            if !seen.insert(key.clone()) {
                return Err(CappedStateProblem::Repeated {
                    instance_id: key.0,
                    pool_id: key.1,
                });
            }
            let Some(restored) = restored_episode(&episode) else {
                return Err(CappedStateProblem::Invalid {
                    instance_id: key.0,
                    pool_id: key.1,
                });
            };
            let valued = catalog.is_some_and(|catalog| {
                catalog
                    .catalog()
                    .pools
                    .pools
                    .iter()
                    .any(|pool| pool.id == key.1 && pool.declared_valuation().is_some())
            });
            if !valued {
                dropped.push(key);
                continue;
            }
            memory.begun.insert(key.clone(), restored.number);
            memory.episodes.insert(key, restored);
        }
        Ok(CappedRestore { memory, dropped })
    }
}

/// The episode `kept` describes, or `None` when it breaks a rule of the shape (module doc).
fn restored_episode(kept: &KeptEpisode) -> Option<Episode> {
    if kept.instance_id.is_empty() || kept.pool_id.is_empty() || kept.number == 0 {
        return None;
    }
    let mut consumers = Vec::with_capacity(kept.consumers.len());
    let mut consumer_ids = BTreeSet::new();
    for consumer in &kept.consumers {
        if consumer.task_id.is_empty()
            || consumer.consumed == 0
            || !consumer_ids.insert(consumer.task_id.as_str())
        {
            return None;
        }
        consumers.push(ConsumerKey {
            task_id: consumer.task_id.clone(),
            consumed: consumer.consumed,
            limited: consumer.limited,
        });
    }
    if consumers.is_empty() {
        return None;
    }
    consumers.sort();
    let round = match &kept.round {
        None => None,
        Some(round) => {
            if kept.rounds == 0 || round.order.is_empty() {
                return None;
            }
            let mut order = Vec::with_capacity(round.order.len());
            let mut passed = BTreeMap::new();
            let mut member_ids = BTreeSet::new();
            for (index, member) in round.order.iter().enumerate() {
                if member.consumed == 0
                    || !consumer_ids.contains(member.task_id.as_str())
                    || !member_ids.insert(member.task_id.as_str())
                    || (!member.passed && member.gate_code.is_some())
                {
                    return None;
                }
                if member.passed {
                    passed.insert(index, member.gate_code);
                }
                order.push(Member {
                    task_id: member.task_id.clone(),
                    consumed: member.consumed,
                });
            }
            Some(Round {
                started_at_unix_ms: round.started_at_unix_ms,
                reading: i128::from(round.reading),
                order,
                passed,
                offered: Vec::new(),
            })
        }
    };
    Some(Episode {
        consumers,
        number: kept.number,
        rounds: kept.rounds,
        round,
    })
}
