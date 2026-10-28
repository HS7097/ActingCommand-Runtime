// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-2b-2 (design P22 §6.2 "SR-2b-2"; `contracts/policy-suspension.md`, "The
//! rerun" and "Limits"; `contracts/scheduling-blackout.md`, "Order of the deferrals", position
//! 6): what the policy evaluation does with the reruns that condition terminations owe.
//!
//! - **Promotion.** Each owed rerun (`condition_reruns`, register L7) that has not expired enters
//!   the evaluation as triggered and past its cooldown, with the reason `policy_rerun_owed`;
//!   with `scheduling.rerun.promote` it ranks ahead of its instance's other candidates
//!   (`actingcommand_policy::OwedReruns`). Every other gate applies to it as to any candidate.
//! - **Not after exhaustion (coordinator decision on the SR-2b-1 review's M1).** A pair whose
//!   condition terminations were exhausted in an occurrence does not enter as an owed rerun
//!   again in that occurrence: a later owed rerun there waits for the pair's own trigger and
//!   cooldown, so sustained load cannot turn each trigger into `max_per_occurrence + 1` runs.
//!   Memory only, like the exhaustion's warning queue: a restart forgets it.
//! - **Stage gate.** While the instance's effective pacing stage is above
//!   `scheduling.rerun.resume_stage`, or has been back at or below it for less than
//!   `resume_settle_ms`, the owed rerun is deferred with `policy_rerun_waiting{pacing_stage}`
//!   (position 6); the settle's end is its wake, and a stage change wakes the evaluation
//!   (`perf.pacing_changed` raises `ResourcesChanged`). An instance never seen above the resume
//!   stage passes at once, so under `observe` (stage S0) the rerun is let through at once.
//! - **Expiry.** At or past `expires_at_unix_ms` the rerun is no longer owed: it is not let in,
//!   the run's terminal shows in the snapshot again (`task_runtime_snapshots_of`), and
//!   `policy_rerun_expired` (warning) is recorded once.
//! - The first wait of each owed rerun is recorded once as `policy_rerun_waiting` (info), which
//!   `actingd suspended` reads to list it as waiting.
//!
//! Both records use the carrier of `policy_rerun_exhausted`: `policy.planning_signal_observed` of
//! kind `goal_missed`, `signal_id` [`owed_rerun_signal_id`]. Nothing here knows a game.

use crate::RuntimeHostResult;
use crate::codes::SchedulingRuleHostCode;
use crate::policy_control::{RerunGateSettings, owed_rerun_signal_id};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    PerformancePacingStage, PolicyConditionReruns, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind,
};
use actingcommand_policy::{
    CandidateEligibility, DecisionReason, DispatchIntent, FACT_REFRESH_TASK_ID, OwedRerun,
    OwedReruns,
};
use std::collections::{BTreeMap, BTreeSet};

/// What a cycle reads about owed reruns as it starts.
pub(crate) struct RerunContext<'a> {
    /// The `scheduling.rerun` values in effect.
    pub(crate) settings: RerunGateSettings,
    /// Each instance's effective pacing stage as the cycle starts, by alias; S0 for one without
    /// a reading.
    pub(crate) stages: &'a BTreeMap<String, PerformancePacingStage>,
}

/// How an instance's stage stood against the resume stage at the last evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageWatch {
    /// Above the resume stage.
    Above,
    /// Back at or below it since this evaluation time, not yet for `resume_settle_ms`.
    Settling { since_unix_ms: u64 },
}

/// The policy host's memory of owed reruns besides the decision memory.
#[derive(Debug, Default)]
pub(crate) struct RerunState {
    /// Per pair, the window end of the occurrence its condition terminations were exhausted in.
    exhausted: BTreeMap<(String, String), u64>,
    /// Per instance alias, how its stage stands; an instance never above has no entry.
    watch: BTreeMap<String, StageWatch>,
    /// The `signal_id`s of the current owed reruns' records already queued or found recorded.
    signalled: BTreeSet<String>,
}

/// One evaluation's owed reruns: the pairs let in, the stage gate's waits by pair, and the
/// records to write after the evaluation.
#[derive(Debug, Default)]
pub(crate) struct RerunPlan {
    pub(crate) owed: OwedReruns,
    waits: BTreeMap<(String, String), (DecisionReason, Option<u64>)>,
    pub(crate) signals: Vec<PolicyPlanningSignalEventData>,
}

impl RerunPlan {
    /// Position 6: the stage gate's deferral of `intent`'s pair, if it waits.
    pub(crate) fn deferral(&self, intent: &DispatchIntent) -> Option<CandidateEligibility> {
        if self.waits.is_empty() {
            return None;
        }
        self.waits
            .get(&(intent.task_id.clone(), intent.instance_id.clone()))
            .map(|(reason, wake)| CandidateEligibility::Deferred {
                reason: reason.clone(),
                next_wake_unix_ms: *wake,
            })
    }
}

impl RerunState {
    /// The pair's condition terminations were exhausted in the occurrence ending at
    /// `window_end_unix_ms`.
    pub(crate) fn note_exhausted(&mut self, task_id: &str, instance_id: &str, window_end: u64) {
        self.exhausted
            .insert((task_id.to_owned(), instance_id.to_owned()), window_end);
    }

    /// Reads each instance's stage at `now` and moves its watch.
    fn observe(&mut self, context: &RerunContext<'_>, now: u64) {
        let settings = context.settings;
        for (alias, stage) in context.stages {
            if *stage > settings.resume_stage {
                self.watch.insert(alias.clone(), StageWatch::Above);
                continue;
            }
            match self.watch.get(alias).copied() {
                Some(StageWatch::Above) => {
                    self.watch
                        .insert(alias.clone(), StageWatch::Settling { since_unix_ms: now });
                }
                Some(StageWatch::Settling { since_unix_ms })
                    if now.saturating_sub(since_unix_ms) >= settings.resume_settle_ms =>
                {
                    self.watch.remove(alias);
                }
                Some(StageWatch::Settling { .. }) | None => {}
            }
        }
    }

    /// Whether the instance's owed reruns wait: `None` when they pass; otherwise the wake, none
    /// while the stage is above the resume stage (its change wakes the evaluation).
    fn wait(&self, alias: &str, settings: RerunGateSettings) -> Option<Option<u64>> {
        match self.watch.get(alias)? {
            StageWatch::Above => Some(None),
            StageWatch::Settling { since_unix_ms } => Some(Some(
                since_unix_ms.saturating_add(settings.resume_settle_ms),
            )),
        }
    }

    /// The owed reruns of one evaluation at `now` from the loaded decision memory `reruns`.
    /// `recorded` answers whether a record's `signal_id` is already in the planning-signal
    /// projection.
    pub(crate) fn plan(
        &mut self,
        reruns: &BTreeMap<(String, String), PolicyConditionReruns>,
        context: &RerunContext<'_>,
        now: u64,
        mut recorded: impl FnMut(&str) -> RuntimeHostResult<bool>,
    ) -> RuntimeHostResult<RerunPlan> {
        self.observe(context, now);
        self.exhausted.retain(|_, window_end| now < *window_end);
        let settings = context.settings;
        let mut plan = RerunPlan::default();
        let mut signalled = BTreeSet::new();
        let mut signal = |id: String,
                          signal: PolicyPlanningSignalEventData,
                          plan: &mut RerunPlan|
         -> RuntimeHostResult<()> {
            if !self.signalled.contains(&id) && !recorded(&id)? {
                plan.signals.push(signal);
            }
            signalled.insert(id);
            Ok(())
        };
        for ((task_id, instance_id), owed) in reruns {
            // A fact refresh is no catalog task; the evaluation never reads it as one.
            if task_id == FACT_REFRESH_TASK_ID {
                continue;
            }
            if now >= owed.expires_at_unix_ms {
                let id = owed_rerun_signal_id("expired", task_id, instance_id, owed);
                let outcome = with_failure_code(
                    Outcome::new(SchedulingRuleHostCode::PolicyRerunExpired)
                        .task_id(task_id.as_str())
                        .instance_id(instance_id.as_str()),
                    owed,
                )
                .window_end_unix_ms(owed.expires_at_unix_ms);
                signal(
                    id.clone(),
                    planning_signal(id, task_id, instance_id, outcome, now),
                    &mut plan,
                )?;
                continue;
            }
            let pair = (task_id.clone(), instance_id.clone());
            if self.exhausted.get(&pair) == Some(&owed.expires_at_unix_ms) {
                continue;
            }
            let owed_outcome = with_failure_code(
                Outcome::new(SchedulingRuleHostCode::PolicyRerunOwed)
                    .task_id(task_id.as_str())
                    .instance_id(instance_id.as_str()),
                owed,
            )
            .count(owed.count);
            plan.owed.pairs.insert(
                pair.clone(),
                OwedRerun {
                    reason: DecisionReason {
                        outcome: owed_outcome,
                        detail: format!(
                            "a rerun is owed: the run ended by a condition ({}), condition termination {} of the occurrence ending at {}",
                            owed.failure_code.as_str(),
                            owed.count,
                            owed.expires_at_unix_ms
                        ),
                    },
                    promote: settings.promote,
                },
            );
            let Some(wake) = self.wait(instance_id, settings) else {
                continue;
            };
            let stage = context
                .stages
                .get(instance_id)
                .copied()
                .unwrap_or(PerformancePacingStage::S0);
            let mut waiting = Outcome::new(SchedulingRuleHostCode::PolicyRerunWaiting)
                .instance_id(instance_id.as_str())
                .pacing_stage(stage);
            if let Some(wake) = wake {
                waiting = waiting.next_eligible_unix_ms(wake);
            }
            let id = owed_rerun_signal_id("waiting", task_id, instance_id, owed);
            signal(
                id.clone(),
                planning_signal(id, task_id, instance_id, waiting.clone(), now),
                &mut plan,
            )?;
            plan.waits.insert(
                pair,
                (
                    DecisionReason {
                        outcome: waiting,
                        detail: format!(
                            "the owed rerun waits for the pacing stage: {} now, it runs at or below {} held for {} ms",
                            stage.as_str(),
                            settings.resume_stage.as_str(),
                            settings.resume_settle_ms
                        ),
                    },
                    wake,
                ),
            );
        }
        self.signalled = signalled;
        Ok(plan)
    }
}

/// `outcome` with the owed rerun's `failure_code`; a code the catalog no longer lists stays
/// visible as its `caused_by` link.
fn with_failure_code(outcome: Outcome, owed: &PolicyConditionReruns) -> Outcome {
    match owed.failure_code.registered() {
        Some(code) => outcome.failure_code(code),
        None => outcome.caused_by(Outcome::relayed(owed.failure_code.as_str())),
    }
}

fn planning_signal(
    signal_id: String,
    task_id: &str,
    instance_id: &str,
    outcome: Outcome,
    now: u64,
) -> PolicyPlanningSignalEventData {
    PolicyPlanningSignalEventData {
        signal_id,
        instance_id: instance_id.to_owned(),
        task_id: Some(task_id.to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome,
        observed_at_unix_ms: now,
        detection_budget: None,
    }
}
