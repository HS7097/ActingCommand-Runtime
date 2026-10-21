// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B41 (coordinator triage Q15, frozen model items 1 to 5): a chain step that hands
//! over on its page goes first.
//!
//! - A *handover* is a round in which a chain's predecessor run ended completed on a page in the
//!   successor's entry page set, and the successor is due: a `dependency_completed` its trigger
//!   names (outside a `not`, accepting `succeeded`) names the predecessor, the predecessor's
//!   latest run on the instance succeeded, the page the instance rests on was recognized during
//!   that run (nothing recognized another page since it ended), the successor's package entry
//!   holds that page (the Runtime answers, it reads the package), and the successor is a
//!   candidate of the round.
//! - On that instance the successor goes first: it is allocated before every candidate that
//!   ranked ahead of it, and while it is dispatched or not yet allocated each of those waits
//!   under `policy_chain_handover_waiting`, a fact refresh reading included (its run would take
//!   the instance off the page before the successor). The Runtime does not predict where another
//!   package ends, so every one of them waits.
//! - The waiting work keeps its own due time. It runs after the chain step, or in the same round
//!   when the successor is not dispatched (an admission gate or the host budget defers it), and
//!   from the round the successor is no longer a candidate.
//! - A successor whose own trigger or selection document reads a key the fact refresh reads is
//!   deferred for that key and is no candidate (item 4): the refresh goes first, as before.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;

use super::{DecisionReason, PlacementCandidate, TaskRuntimeSnapshot, reason_with};
use crate::codes::PolicyCode;
use crate::{PredicateSpec, TaskSpec, TaskTerminalState};

/// The page an instance rests on: the page its latest recognition matched, as the Runtime
/// recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestingPage {
    /// The page id the recognition matched.
    pub page_id: String,
    /// When the Runtime recorded it.
    pub observed_at_unix_ms: u64,
}

/// Workflow #411 B41: what one evaluation's chain handover reads besides the evaluation inputs.
/// Memory only: none of it enters a snapshot or a decision identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainHandoverInputs {
    /// Per instance alias, the page it rests on; an instance without one has no handover.
    pub resting_pages: BTreeMap<String, RestingPage>,
}

/// The Runtime's answer whether the entry page set of a task's package holds a page, given the
/// task, the instance alias and the page id.
pub type ChainEntryHolds<'a, E> = dyn FnMut(&TaskSpec, &str, &str) -> Result<bool, E> + 'a;

/// How far the allocation got with an instance's successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allocation {
    /// The successor was not allocated yet.
    Pending,
    /// The successor was dispatched this round.
    Dispatched,
    /// The successor was not dispatched this round: the candidates it went before take their
    /// turn.
    NotDispatched,
}

/// One instance's handover in this round.
#[derive(Debug)]
pub(super) struct Handover {
    successor_work_index: usize,
    successor_task_id: String,
    page_id: String,
    /// The candidates of the instance that ranked ahead of the successor.
    ahead: BTreeSet<usize>,
    allocation: Allocation,
}

impl Handover {
    /// Whether `work_index` is the instance's successor.
    pub(super) fn is_successor(&self, work_index: usize) -> bool {
        work_index == self.successor_work_index
    }

    /// Whether the candidate `work_index` waits for the successor now: it ranked ahead of it, and
    /// the successor was dispatched or is still to be allocated.
    pub(super) fn holds(&self, work_index: usize) -> bool {
        self.ahead.contains(&work_index) && self.allocation != Allocation::NotDispatched
    }

    /// Records whether the allocation dispatched the successor.
    pub(super) fn settle(&mut self, dispatched: bool) {
        self.allocation = if dispatched {
            Allocation::Dispatched
        } else {
            Allocation::NotDispatched
        };
    }

    /// `policy_chain_handover_waiting`: the candidate waits for the chain step that goes first on
    /// `instance_id` (keys `instance_id`, `task_id` the successor, `page_id` the page it starts
    /// on). No wake of its own: the successor's dispatch and settlement wake the evaluation.
    pub(super) fn waiting_reason(&self, instance_id: &str) -> DecisionReason {
        reason_with(
            Outcome::new(PolicyCode::PolicyChainHandoverWaiting)
                .instance_id(instance_id)
                .task_id(self.successor_task_id.as_str())
                .page_id(self.page_id.as_str()),
            format!(
                "instance_id={instance_id} task_id={} page_id={}",
                self.successor_task_id, self.page_id
            ),
        )
    }
}

/// Finds each instance's handover among the ranked `candidates`, the first candidate in rank
/// order that hands over, and moves its successor ahead of every candidate of its instance.
pub(super) fn plan<E>(
    inputs: &ChainHandoverInputs,
    entry_holds: &mut ChainEntryHolds<'_, E>,
    tasks: &[TaskSpec],
    task_states: &BTreeMap<(&str, &str), &TaskRuntimeSnapshot>,
    candidates: &mut Vec<PlacementCandidate>,
) -> Result<BTreeMap<String, Handover>, E> {
    let mut handovers = BTreeMap::<String, Handover>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if handovers.contains_key(&candidate.instance_id) {
            continue;
        }
        let Some(page) = inputs.resting_pages.get(&candidate.instance_id) else {
            continue;
        };
        // A fact refresh reading is no catalog task, so it follows no predecessor.
        let Some(task) = tasks.iter().find(|task| task.id == candidate.task_id) else {
            continue;
        };
        // The Runtime reads the successor's package only for a candidate that follows its
        // predecessor on the resting page.
        let hands_over = follows_on_page(task, &candidate.instance_id, page, task_states)
            && entry_holds(task, &candidate.instance_id, &page.page_id)?;
        if !hands_over {
            continue;
        }
        let ahead = candidates[..index]
            .iter()
            .filter(|other| other.instance_id == candidate.instance_id)
            .map(|other| other.work_index)
            .collect();
        handovers.insert(
            candidate.instance_id.clone(),
            Handover {
                successor_work_index: candidate.work_index,
                successor_task_id: task.id.clone(),
                page_id: page.page_id.clone(),
                ahead,
                allocation: Allocation::Pending,
            },
        );
    }
    // B41 red proof 2 (mutation): the deferral applies unconditionally, also when the successor
    // is no candidate because it reads the key the fact refresh reads.
    for (instance_id, page) in &inputs.resting_pages {
        if handovers.contains_key(instance_id) {
            continue;
        }
        for task in tasks {
            if follows_on_page(task, instance_id, page, task_states)
                && entry_holds(task, instance_id, &page.page_id)?
            {
                let ahead = candidates
                    .iter()
                    .filter(|candidate| candidate.instance_id == *instance_id)
                    .map(|candidate| candidate.work_index)
                    .collect();
                handovers.insert(
                    instance_id.clone(),
                    Handover {
                        successor_work_index: usize::MAX,
                        successor_task_id: task.id.clone(),
                        page_id: page.page_id.clone(),
                        ahead,
                        allocation: Allocation::Pending,
                    },
                );
                break;
            }
        }
    }
    for handover in handovers.values() {
        let first = candidates
            .iter()
            .position(|candidate| handover.ahead.contains(&candidate.work_index));
        let successor = candidates
            .iter()
            .position(|candidate| handover.is_successor(candidate.work_index));
        if let (Some(first), Some(successor)) = (first, successor) {
            let moved = candidates.remove(successor);
            candidates.insert(first, moved);
        }
    }
    Ok(handovers)
}

/// Whether `task` follows, on `instance_id`, a predecessor whose latest run there ended
/// completed while the instance rests on the page that run recognized: the resting page was
/// recorded between the run's admission and its end.
fn follows_on_page(
    task: &TaskSpec,
    instance_id: &str,
    page: &RestingPage,
    task_states: &BTreeMap<(&str, &str), &TaskRuntimeSnapshot>,
) -> bool {
    let mut predecessors = Vec::new();
    predecessors_of(&task.trigger, &mut predecessors);
    predecessors.into_iter().any(|predecessor| {
        task_states
            .get(&(predecessor, instance_id))
            .is_some_and(|state| {
                state.terminal_state == Some(TaskTerminalState::Succeeded)
                    && state.last_dispatched_unix_ms.is_some_and(|admitted| {
                        state.completed_window.as_ref().is_some_and(|completed| {
                            (admitted..=completed.completed_at_unix_ms)
                                .contains(&page.observed_at_unix_ms)
                        })
                    })
            })
    })
}

/// The tasks a trigger follows: every `dependency_completed` that accepts `succeeded`, reached
/// through `all` and `any`; a `not` names none.
fn predecessors_of<'a>(predicate: &'a PredicateSpec, found: &mut Vec<&'a str>) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for predicate in predicates {
                predecessors_of(predicate, found);
            }
        }
        PredicateSpec::DependencyCompleted {
            task_id,
            terminal_states,
        } if terminal_states.contains(&TaskTerminalState::Succeeded) => {
            found.push(task_id.as_str());
        }
        _ => {}
    }
}
