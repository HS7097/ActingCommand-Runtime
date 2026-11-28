// SPDX-License-Identifier: AGPL-3.0-only

//! #503 PACE (0.12.100; `contracts/policy-suspension.md`, "A pair whose runs keep failing";
//! `contracts/scheduling-blackout.md`, "Order of the deferrals", position 4b; register §1.545): a
//! pair (instance, catalog task) whose scheduled runs keep failing waits a growing interval between
//! its dispatches from its second failure in a row on, and is set aside for its activity window once
//! it keeps failing although its instance lets other scheduled runs succeed.
//!
//! A failure that does not accumulate (its identity's `F` unique) never repeats the previous
//! identity, so its count stays 1: with a `retry_limit` of 1 or more every such failure schedules
//! an immediate rerun again (R22), and with none the pair is a candidate again at the next
//! evaluation while its trigger holds. Only the counts of its rule and its activity bounded it.
//! This bounds it with what counts no run of a day, as Workflow #490 ENTRY-A bounds a pair that
//! keeps failing at its entry, in the simplest form: no block, no stretch, no turn order.
//!
//! - **Which settlements count.** The live settlement of a scheduled run whose execution record is
//!   a failure with no `condition` and whose disposition leaves the pair unpaused
//!   (`retry_scheduled` or `continue`). A stop, a failure a condition settled (a configured one or
//!   one at the entry, each with its own pacing), a reconciled or interrupted settlement leave the
//!   pair as it is; a store update package's task is neither counted nor held.
//! - **The count in a row and the wait.** A failure is one more in a row when the pair's previous
//!   one was in the same activity window, else the first. After the first nothing is added (R22:
//!   the first rerun is as immediate as before). After the n-th, n ≥ 2, the pair is not
//!   dispatched on its instance before `scheduling.unavailable_probe`'s interval for n − 1 has
//!   passed since that failure's settlement, never past the window's end
//!   (`instance_task_failure_waiting`, the candidate's reason; the same code is the one record
//!   under the instance that the settlement setting the wait writes, never an evaluation).
//! - **Set aside.** A failure after the first in the row counts against the pair when a scheduled
//!   run (not a store update task's) succeeded on the instance since the pair's previous failure
//!   and its record is not `performance_tax_exempt`; once that count reaches the rule's
//!   `escalation_threshold` and the window is known the pair waits until the window's end
//!   (`instance_task_failure_set_aside`) and a person is told once
//!   (`instance_task_failure_set_aside_reported`).
//! - **Its end.** A success of the pair, a settlement that pauses it, the window's end, a
//!   catalogue's activation, the scheduler workbench's changed package, a store update task's
//!   success on the instance (every pair of it) and a restart (memory only).
//!
//! The host keeps the pairs (`host/failure_pairs.rs`) and hands the evaluation each pair by alias
//! as the cycle's inputs are read; this is the memory and what the evaluation answers. The Runtime
//! names no game, package or page here.

use crate::codes::HostCode;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{
    InstanceId, PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureDisposition,
};
use actingcommand_policy::{CandidateEligibility, DecisionReason};
use actingcommand_runtime_config::UnavailableProbeConfig;
use std::collections::BTreeMap;

/// What the policy knows of a settled scheduled run's dispatch and task, which its pair reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PacingRule {
    /// The end of the activity window the run's admission was committed in (`0`: unknown).
    pub(crate) window_end: u64,
    /// The task's `on_failure.escalation_threshold` in the dispatch's catalog generation
    /// (`u16::MAX` when the generation holds no such task: nothing is set aside then).
    pub(crate) threshold: u16,
    /// The task is a store update package's: its runs are neither counted nor held.
    pub(crate) store_task: bool,
}

/// What one live settlement of a scheduled run tells its pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PacedSettlement {
    /// A success: the pair starts from nothing, and (but for a store update task's) its instance
    /// counts one more scheduled run that succeeded.
    Succeeded,
    /// A failure with no condition whose disposition leaves the pair unpaused: it counts.
    Failed {
        /// The run's dispatch.
        decision_id: String,
        /// Its failure's code.
        failure_code: CodeStr,
        /// Its execution record's observation time: its settlement.
        observed_at_unix_ms: u64,
        /// Its record is `performance_tax_exempt`: it never counts against the pair.
        exempt: bool,
    },
    /// A failure whose disposition pauses the pair: the pair starts from nothing.
    Paused,
    /// A stop or a failure a condition settled: the pair stays as it is.
    Untouched,
}

impl PacedSettlement {
    /// What the execution record `data`, recorded now by a live settlement, tells its pair.
    pub(crate) fn of(data: &PolicyExecutionEventData) -> Self {
        match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => Self::Succeeded,
            PolicyExecutionOutcome::Stopped { .. } => Self::Untouched,
            PolicyExecutionOutcome::Failed { failure } if failure.condition.is_some() => {
                Self::Untouched
            }
            PolicyExecutionOutcome::Failed { failure } => {
                if failure.disposition == PolicyFailureDisposition::PausedTask {
                    Self::Paused
                } else {
                    Self::Failed {
                        decision_id: data.decision_id.clone(),
                        failure_code: actingcommand_contract::scheduled_failure_code_str(
                            &failure.outcome,
                        ),
                        observed_at_unix_ms: data.observed_at_unix_ms,
                        exempt: failure.performance_tax_exempt,
                    }
                }
            }
        }
    }
}

/// One pair whose latest scheduled runs failed, as the host keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailurePair {
    /// The end of the activity window its failures' admissions were committed in (`0`: unknown).
    pub(crate) window_end: u64,
    /// The latest failure: its dispatch, its code and its execution record's observation time.
    pub(crate) decision_id: String,
    pub(crate) failure_code: CodeStr,
    pub(crate) observed_at_unix_ms: u64,
    /// Its failures in a row in the window, and the time before which it is not dispatched on its
    /// instance (`0` after the first).
    pub(crate) in_a_row: u32,
    pub(crate) until_unix_ms: u64,
    /// The scheduled runs that had succeeded on its instance when the latest failure settled, and
    /// the failures that came although that count had grown since the pair's previous one.
    pub(crate) succeeded: u64,
    pub(crate) against: u32,
    /// The rule's `escalation_threshold`, and whether `against` reached it with the window known,
    /// which sets the pair aside until `window_end`.
    pub(crate) threshold: u16,
    pub(crate) set_aside: bool,
}

impl FailurePair {
    /// The end of its wait as the evaluation reads it: never past its window's end.
    pub(crate) fn wait_end(&self) -> u64 {
        wait_end(self.until_unix_ms, self.window_end)
    }
}

/// The end of a wait until `until_unix_ms` in the activity window ending at `window_end` (`0`:
/// unknown): never past the window's end.
fn wait_end(until_unix_ms: u64, window_end: u64) -> u64 {
    if window_end > 0 {
        until_unix_ms.min(window_end)
    } else {
        until_unix_ms
    }
}

/// What a settlement tells a person of its pair (the owner's ruling on this slice: the situation
/// has its code, which says the result and the operation that follows): one record
/// under the instance for each failure that sets a wait, or, in its place, for the failure that
/// sets the pair aside. The first failure in a row writes none: its execution record already says
/// its rerun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PacedRecord {
    /// The failure, the second in a row or later, set a wait (`instance_task_failure_waiting`).
    Waits(FailurePair),
    /// The failure set the pair aside for its window
    /// (`instance_task_failure_set_aside_reported`).
    SetAside(FailurePair),
}

/// The pairs whose scheduled runs keep failing, and the scheduled runs that succeeded on each
/// instance (memory only; a restart starts with none).
#[derive(Debug, Default)]
pub(crate) struct FailurePairs {
    succeeded: BTreeMap<InstanceId, u64>,
    pairs: BTreeMap<(InstanceId, String), FailurePair>,
}

impl FailurePairs {
    /// The pair of `task_id` on `instance_id` settled as `settlement` under `rule`; `config` gives
    /// the intervals. Returns the record the settlement writes: the wait a failure after the first
    /// in a row set, or the pair's being set aside, said once per window.
    pub(crate) fn settle(
        &mut self,
        instance_id: InstanceId,
        task_id: &str,
        settlement: PacedSettlement,
        rule: PacingRule,
        config: UnavailableProbeConfig,
    ) -> Option<PacedRecord> {
        if rule.store_task {
            // A store update task's run is neither counted nor held; its success starts every
            // pair of its instance from nothing (the game is another version) and is no evidence
            // that the instance lets tasks succeed.
            if matches!(settlement, PacedSettlement::Succeeded) {
                self.pairs
                    .retain(|(pair_instance, _), _| *pair_instance != instance_id);
            }
            return None;
        }
        let key = (instance_id, task_id.to_owned());
        match settlement {
            PacedSettlement::Succeeded => {
                let counted = self.succeeded.entry(instance_id).or_insert(0);
                *counted = counted.saturating_add(1);
                self.pairs.remove(&key);
                None
            }
            PacedSettlement::Paused => {
                self.pairs.remove(&key);
                None
            }
            PacedSettlement::Untouched => None,
            PacedSettlement::Failed {
                decision_id,
                failure_code,
                observed_at_unix_ms,
                exempt,
            } => {
                let succeeded = self.succeeded.get(&instance_id).copied().unwrap_or(0);
                let (in_a_row, against, was_set_aside) = self
                    .pairs
                    .get(&key)
                    .filter(|pair| pair.window_end == rule.window_end)
                    .map_or((1, 0, false), |pair| {
                        (
                            pair.in_a_row.saturating_add(1),
                            pair.against
                                .saturating_add(u32::from(!exempt && succeeded > pair.succeeded)),
                            pair.set_aside,
                        )
                    });
                let set_aside =
                    rule.window_end > 0 && against > 0 && against >= u32::from(rule.threshold);
                let until_unix_ms = if in_a_row > 1 {
                    observed_at_unix_ms
                        .saturating_add(config.interval_ms(in_a_row.saturating_sub(1)))
                } else {
                    0
                };
                let pair = FailurePair {
                    window_end: rule.window_end,
                    decision_id,
                    failure_code,
                    observed_at_unix_ms,
                    in_a_row,
                    until_unix_ms,
                    succeeded,
                    against,
                    threshold: rule.threshold,
                    set_aside,
                };
                let record = if set_aside {
                    // In place of the wait's record: a pair set aside waits for its window's end.
                    (!was_set_aside).then(|| PacedRecord::SetAside(pair.clone()))
                } else {
                    (in_a_row > 1).then(|| PacedRecord::Waits(pair.clone()))
                };
                self.pairs.insert(key, pair);
                record
            }
        }
    }

    /// Every pair starts from nothing: a catalogue became active, or the scheduler workbench's
    /// check found a package of a game in use changed.
    pub(crate) fn clear(&mut self) {
        self.pairs.clear();
    }

    /// The pairs, by instance and catalog task.
    pub(crate) fn pairs(&self) -> &BTreeMap<(InstanceId, String), FailurePair> {
        &self.pairs
    }

    /// Test only: the pair of `task_id` on `instance_id`, if it has one.
    #[cfg(test)]
    pub(crate) fn pair(&self, instance_id: InstanceId, task_id: &str) -> Option<&FailurePair> {
        self.pairs.get(&(instance_id, task_id.to_owned()))
    }

    /// Test only: `pair` stands for `task_id` on `instance_id`, in place of its own.
    #[cfg(test)]
    pub(crate) fn seed(&mut self, instance_id: InstanceId, task_id: &str, pair: FailurePair) {
        self.pairs.insert((instance_id, task_id.to_owned()), pair);
    }
}

/// One pair as the evaluation reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailurePacingGate {
    /// The pair's latest failure: its dispatch, its code and its settlement.
    pub(crate) decision_id: String,
    pub(crate) failure_code: CodeStr,
    pub(crate) observed_at_unix_ms: u64,
    /// Its failures in a row in the window.
    pub(crate) in_a_row: u32,
    /// The pair is not dispatched on the instance before this time (`0`: no wait).
    pub(crate) until_unix_ms: u64,
    /// The end of the activity window its failures were in (`0`: unknown).
    pub(crate) window_end_unix_ms: u64,
    /// Set aside until the window's end, with the failures against it and the rule's
    /// `escalation_threshold`.
    pub(crate) set_aside: Option<(u32, u16)>,
}

impl FailurePacingGate {
    /// The gate of `pair`.
    pub(crate) fn of(pair: &FailurePair) -> Self {
        Self {
            decision_id: pair.decision_id.clone(),
            failure_code: pair.failure_code.clone(),
            observed_at_unix_ms: pair.observed_at_unix_ms,
            in_a_row: pair.in_a_row,
            until_unix_ms: pair.until_unix_ms,
            window_end_unix_ms: pair.window_end,
            set_aside: pair.set_aside.then_some((pair.against, pair.threshold)),
        }
    }
}

/// The pairs that may wait or be set aside, by instance alias and catalog task, as the evaluation
/// reads them.
#[derive(Debug, Clone, Default)]
pub(crate) struct FailurePacingGates {
    pairs: BTreeMap<(String, String), FailurePacingGate>,
}

impl FailurePacingGates {
    pub(crate) fn new(pairs: BTreeMap<(String, String), FailurePacingGate>) -> Self {
        Self { pairs }
    }

    /// Position 4b, after the entry's pairs: the deferral of the task `task_id`'s candidate on the
    /// instance `instance_id` (its alias) at `now_unix_ms`, when its pair is set aside until its
    /// activity window's end (`instance_task_failure_set_aside`) or waits after its latest failure
    /// (`instance_task_failure_waiting`); each wakes at its end, and neither holds once the window
    /// has ended. `None` for any other candidate. A fact refresh's names the code as its gate.
    pub(crate) fn deferral(
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
                let code = HostCode::InstanceTaskFailureSetAside;
                let outcome = pair_outcome(code)
                    .count(count)
                    .limit_count(threshold)
                    .window_end_unix_ms(window_end);
                (code, outcome, window_end)
            }
            _ => {
                let until = wait_end(pair.until_unix_ms, window_end);
                if now_unix_ms >= until {
                    return None;
                }
                let code = HostCode::InstanceTaskFailureWaiting;
                let outcome = pair_outcome(code)
                    .count(pair.in_a_row)
                    .next_eligible_unix_ms(until);
                (code, outcome, until)
            }
        };
        let reason = DecisionReason {
            outcome,
            detail: format!("Runtime instance task failure paced: instance_id={instance_id}"),
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
