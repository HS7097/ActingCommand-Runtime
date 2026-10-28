// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned failure, activity-window, and loop-budget state.

use crate::codes::{HostCode, HostLocation};
use crate::error::spelled;
use crate::failure_identity::FailureIdentity;
use crate::suspension_lift::{PausedPackages, SuspensionLiftView};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Code, CodeStr, Location, Outcome};
use actingcommand_contract::{
    MAX_POLICY_CONTROL_STATE_DAYS, PerformanceContext, PolicyActivitySample,
    PolicyActivityStateRecord, PolicyAdmissionRecord, PolicyBudgetDenial, PolicyBudgetDimension,
    PolicyBudgetReceipt, PolicyConditionReruns, PolicyConditionSettlement,
    PolicyControlStateRecord, PolicyDailyCount, PolicyDispatchClockRecord,
    PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition,
    PolicyFailureRecord, PolicyFailureStreakRecord, PolicyJournalDispatchRecord,
    PolicyRetryRoundRecord, RuntimeErrorCode, scheduled_failure_code_str,
};
use actingcommand_policy::{
    ActivityProfile, CompiledCatalog, DispatchIntent, FailureAction, TaskSpec, activity_window_at,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MILLIS_PER_MINUTE: i128 = 60_000;
const MILLIS_PER_DAY: i128 = 24 * 60 * MILLIS_PER_MINUTE;

/// Workflow #361 M6, review P5: whether a paused failure is the first interrupted settlement
/// of its pair in a row (`policy_settlement_interrupted`, recorded by the startup
/// reconciliation in `policy_outcome.rs`), which the restart that recorded it lifts. The count
/// is rebuilt by replay, so nothing persisted changes.
pub(crate) fn interruption_lifted_by_restart(failure_code: &CodeStr, consecutive: u16) -> bool {
    *failure_code == SharedCode::PolicySettlementInterrupted.code() && consecutive == 1
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyExecutionInput {
    Succeeded,
    /// A failed run (Workflow #378 A2-P): its failure as the policy records it (see
    /// `PolicyFailureRecord`), the #336 failure identity of a live settlement (a `linear_steps`
    /// or, Workflow #390 EXEC-3, a page-graph run's),
    /// and the run's class.
    Failed {
        outcome: Outcome,
        failure_identity: Option<String>,
        class: PolicyFailureClass,
    },
}

impl PolicyExecutionInput {
    /// Whether `failure` records this input's failure: the same failure code
    /// ([`scheduled_failure_code_str`]), the same failure identity and the same class. The
    /// outcomes' other values are not compared, as a failure streak does not compare them.
    pub(crate) fn names_failure(&self, failure: &PolicyFailureRecord) -> bool {
        match self {
            Self::Succeeded => false,
            Self::Failed {
                outcome,
                failure_identity,
                class,
            } => {
                failure.original_class == *class
                    && failure.failure_identity == *failure_identity
                    && scheduled_failure_code_str(&failure.outcome)
                        == scheduled_failure_code_str(outcome)
            }
        }
    }
}

/// Workflow #386 SR-2b-1 (`contracts/policy-suspension.md`, "Condition termination and owed
/// reruns"): the `scheduling.rerun` values in effect that a run's first settlement decides its
/// `condition` by. `expires` has one value, the end of the pair's bookkeeping window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConditionRule {
    condition_codes: BTreeSet<String>,
    max_per_occurrence: u32,
    refund_counts: bool,
}

impl ConditionRule {
    pub(crate) fn from_config(rerun: &actingcommand_runtime_config::RerunConfig) -> Self {
        match rerun.expires {
            actingcommand_runtime_config::RerunExpiry::WindowEnd => {}
        }
        Self {
            condition_codes: rerun.condition_codes.iter().cloned().collect(),
            max_per_occurrence: rerun.max_per_occurrence,
            refund_counts: rerun.refund_counts,
        }
    }
}

/// Workflow #386 SR-2b-1: how a run's first settlement ends it, from its failure code, the
/// [`ConditionRule`] and its pair's condition terminations in this occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConditionDecision {
    /// No condition: a success, a severe failure, a failure of a sensitive task or a code the
    /// rule does not list.
    Ordinary,
    /// The run ended by a condition; its record carries `condition`.
    Condition(PolicyConditionSettlement),
    /// The pair's `count`-th condition termination in this occurrence, more than the rule's
    /// `limit` allows: the run settles as an ordinary recoverable failure and
    /// `policy_rerun_exhausted` (warning) is recorded. Workflow #386 SR-2b-2: `window_end_unix_ms`
    /// is the end of the occurrence it was exhausted in, so a later owed rerun of the pair in
    /// the same occurrence is not promoted (coordinator decision on review M1).
    Exhausted {
        count: u32,
        limit: u32,
        window_end_unix_ms: u64,
    },
}

/// Workflow #386 SR-2b-2 (`contracts/policy-suspension.md`, "The rerun"): the `scheduling.rerun`
/// values in effect that gate and rank an owed rerun, read at each policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RerunGateSettings {
    /// The highest pacing stage at which an owed rerun runs (`resume_stage`).
    pub(crate) resume_stage: actingcommand_contract::PerformancePacingStage,
    /// How long the stage must stay at or below `resume_stage` after it was above it.
    pub(crate) resume_settle_ms: u64,
    /// Whether an owed rerun ranks ahead of its instance's other candidates.
    pub(crate) promote: bool,
}

impl RerunGateSettings {
    pub(crate) fn from_config(rerun: &actingcommand_runtime_config::RerunConfig) -> Self {
        use actingcommand_contract::PerformancePacingStage as Stage;
        use actingcommand_runtime_config::scheduling::PacingStage;
        Self {
            resume_stage: match rerun.resume_stage {
                PacingStage::S0 => Stage::S0,
                PacingStage::S1 => Stage::S1,
                PacingStage::S2 => Stage::S2,
                PacingStage::S3 => Stage::S3,
                PacingStage::S4 => Stage::S4,
            },
            resume_settle_ms: rerun.resume_settle_ms,
            promote: rerun.promote,
        }
    }
}

/// Workflow #386 SR-2b-2: the `signal_id` of a `policy_rerun_<kind>` record of one owed rerun:
/// `signal:policy_rerun:<kind>:<hex>`, `<hex>` the lowercase SHA-256 of the task, the instance,
/// the owed rerun's window end and its count, so each owed rerun records each kind once.
pub(crate) fn owed_rerun_signal_id(
    kind: &str,
    task_id: &str,
    instance_id: &str,
    owed: &PolicyConditionReruns,
) -> String {
    let expires = owed.expires_at_unix_ms.to_string();
    let count = owed.count.to_string();
    let parts: [&[u8]; 7] = [
        task_id.as_bytes(),
        b"\0",
        instance_id.as_bytes(),
        b"\0",
        expires.as_bytes(),
        b"\0",
        count.as_bytes(),
    ];
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    format!("signal:policy_rerun:{kind}:{:x}", hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyExecutionTiming {
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) runtime_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FailureStreak {
    /// The failure's code ([`scheduled_failure_code_str`]) and its #336 failure identity: with
    /// the class, what makes a failure the same error as the previous one.
    failure_code: CodeStr,
    failure_identity: Option<String>,
    class: PolicyFailureClass,
    count: u16,
    escalation_streak: u16,
    disposition: PolicyFailureDisposition,
    retry_at_unix_ms: Option<u64>,
    /// Workflow #336 L6 (§12.5, §12.7): the dispatch whose execution recorded this failure.
    decision_id: String,
    /// Set only while the pair is paused: what the suspension is lifted against. Workflow #336
    /// PKG-S3: the packages and content hashes its failure identity names (none without one).
    /// Memory only; replay rebuilds it from the execution record alone.
    paused_on: Option<PausedPackages>,
}

/// The normal admission that opened a retry round. Rebuilt in ledger order, never from
/// restart time or a later failure identity. Limits belong to that admission's cycle.
struct RetryRound {
    admission: PolicyAdmissionRecord,
    until_unix_ms: u64,
    expected_duration_ms: u64,
    /// SCH-M ED-2: the task spec's declared duration when the round opened, which a later
    /// catalog's must keep for the round to go on; the round's own duration may be the pair's
    /// expected duration E. Memory only: a round loaded from a value record has none and its
    /// own duration is compared, as before ED-2.
    declared_duration_ms: Option<u64>,
    /// Workflow #372 K5: the order in which the round was opened among all rounds, so a task
    /// reads the latest round of its accounting group. Memory only; replay rebuilds it.
    opened: u64,
}

#[derive(Default)]
pub(crate) struct PolicyControlState {
    task_daily: BTreeMap<(String, String, i64), u32>,
    task_window: BTreeMap<(String, String, String), u32>,
    task_runtime: BTreeMap<(String, String, String), u64>,
    activity_daily: BTreeMap<(String, String, i64), u32>,
    activity_window: BTreeMap<(String, String, String), u32>,
    activity_runtime: BTreeMap<(String, String, String), u64>,
    next_activity_eligible: BTreeMap<(String, String), u64>,
    failure_streaks: BTreeMap<(String, String), FailureStreak>,
    retry_rounds: BTreeMap<(String, String), RetryRound>,
    /// Workflow #372 K5: per (task, instance), the order of the latest settlement that set or
    /// cleared the pair's failure state (a success or a failure a condition did not settle), so
    /// a task reads the failure state of its accounting group's latest such run. Memory only;
    /// replay rebuilds it in ledger order.
    failure_settled: BTreeMap<(String, String), u64>,
    /// Workflow #372 K5: the counters behind `failure_settled` and `RetryRound::opened`.
    settlements: u64,
    rounds_opened: u64,
    /// Workflow #422 LN-3 (register L7): per (task, instance), the activity window of the pair's
    /// latest admission, the one its value record's window counts and runtime count in.
    task_active_window: BTreeMap<(String, String), String>,
    /// Workflow #422 LN-3 (register L14): per (activity profile, instance), the day and the
    /// window of its latest admission, the ones its value record counts in.
    activity_active: BTreeMap<(String, String), (i64, String)>,
    /// Workflow #386 SR-2b-1 (register L7): per (task, instance), the rerun its latest
    /// condition termination owes: the run's failure code, the pair's consecutive condition
    /// terminations in this occurrence and the end of the occurrence's bookkeeping window. Set
    /// from the execution record's `condition`, so a replay rebuilds it; any other settlement of
    /// the pair removes it. At most one per pair.
    condition_reruns: BTreeMap<(String, String), PolicyConditionReruns>,
}

/// Workflow #422 LN-3: one value record of the decision memory, with its key's complete state
/// (register L7 `policy.control_state_recorded`, L14 `policy.activity_state_recorded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyValueRecord {
    Control(Box<PolicyControlStateRecord>),
    Activity(PolicyActivityStateRecord),
}

impl PolicyValueRecord {
    /// The record's key as the ledger's index names it (`contracts/ledger-observer.md` §11.1):
    /// `decision_memory/<instance>/<task>` for a dispatch pair,
    /// `activity_memory/<instance>/<activity profile>` for an activity state.
    pub(crate) fn index_key(&self) -> String {
        match self {
            Self::Control(state) => {
                format!("decision_memory/{}/{}", state.instance_id, state.task_id)
            }
            Self::Activity(state) => {
                format!("activity_memory/{}/{}", state.instance_id, state.profile_id)
            }
        }
    }
}

impl PolicyControlState {
    /// Workflow #422 LN-3 (register L7, L14): the value record of every key this state holds,
    /// each with the key's complete state. `dispatch_clocks` are the admission times of the
    /// pairs' admitted dispatches that are not completed yet, per (task, instance); the policy
    /// host keeps them. Workflow #422 LN-4 (register §1.184 LN4-7): `dispatches` are the
    /// dispatches the policy host's journal keeps per pair, which its record names.
    pub(crate) fn value_records(
        &self,
        dispatch_clocks: &BTreeMap<(String, String), u64>,
        dispatches: &BTreeMap<(String, String), Vec<PolicyJournalDispatchRecord>>,
    ) -> BTreeMap<String, PolicyValueRecord> {
        let pairs = self
            .task_daily
            .keys()
            .map(|(task, instance, _)| (task, instance))
            .chain(
                self.task_window
                    .keys()
                    .map(|(task, instance, _)| (task, instance)),
            )
            .chain(
                self.task_runtime
                    .keys()
                    .map(|(task, instance, _)| (task, instance)),
            )
            .chain(
                self.task_active_window
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .chain(
                self.failure_streaks
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .chain(
                self.retry_rounds
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .chain(
                dispatch_clocks
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .chain(
                self.failure_settled
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .chain(dispatches.keys().map(|(task, instance)| (task, instance)))
            .chain(
                self.condition_reruns
                    .keys()
                    .map(|(task, instance)| (task, instance)),
            )
            .collect::<BTreeSet<_>>();
        let mut records = BTreeMap::new();
        for (task_id, instance_id) in pairs {
            let pair = (task_id.clone(), instance_id.clone());
            let clock = dispatch_clocks.get(&pair).copied();
            let mut record = self.control_record(task_id, instance_id, clock);
            record.dispatches = dispatches.get(&pair).cloned().unwrap_or_default();
            let record = PolicyValueRecord::Control(Box::new(record));
            records.insert(record.index_key(), record);
        }
        for ((profile_id, instance_id), (local_day, window_id)) in &self.activity_active {
            let window = (profile_id.clone(), instance_id.clone(), window_id.clone());
            let record = PolicyValueRecord::Activity(PolicyActivityStateRecord {
                instance_id: instance_id.clone(),
                profile_id: profile_id.clone(),
                local_day: *local_day,
                daily_count: self
                    .activity_daily
                    .get(&(profile_id.clone(), instance_id.clone(), *local_day))
                    .copied()
                    .unwrap_or(0),
                window_id: window_id.clone(),
                window_count: self.activity_window.get(&window).copied().unwrap_or(0),
                window_runtime_ms: self.activity_runtime.get(&window).copied().unwrap_or(0),
                next_eligible_unix_ms: self
                    .next_activity_eligible
                    .get(&(instance_id.clone(), profile_id.clone()))
                    .copied(),
            });
            records.insert(record.index_key(), record);
        }
        records
    }

    /// One dispatch pair's complete decision memory: its newest day counts (at most
    /// `MAX_POLICY_CONTROL_STATE_DAYS`), its counts and runtime in the window of its latest
    /// admission, its failure streak (`paused_on` is rebuilt from the failure identity), its retry
    /// round and its dispatch clock. Workflow #386 SR-2b-1: `condition_reruns` is the rerun the
    /// pair's latest condition termination owes, as its settlement wrote it.
    fn control_record(
        &self,
        task_id: &str,
        instance_id: &str,
        dispatch_clock: Option<u64>,
    ) -> PolicyControlStateRecord {
        let pair = (task_id.to_owned(), instance_id.to_owned());
        let mut days = self
            .task_daily
            .range(
                (pair.0.clone(), pair.1.clone(), i64::MIN)
                    ..=(pair.0.clone(), pair.1.clone(), i64::MAX),
            )
            .map(|((_, _, local_day), count)| PolicyDailyCount {
                local_day: *local_day,
                count: *count,
            })
            .collect::<Vec<_>>();
        let daily_counts = days.split_off(days.len().saturating_sub(MAX_POLICY_CONTROL_STATE_DAYS));
        let active_window = self.task_active_window.get(&pair).cloned();
        let (window_counts, window_runtime_ms) = active_window.as_ref().map_or((0, 0), |window| {
            let key = (pair.0.clone(), pair.1.clone(), window.clone());
            (
                self.task_window.get(&key).copied().unwrap_or(0),
                self.task_runtime.get(&key).copied().unwrap_or(0),
            )
        });
        PolicyControlStateRecord {
            instance_id: instance_id.to_owned(),
            task_id: task_id.to_owned(),
            daily_counts,
            window_counts,
            window_runtime_ms,
            active_window,
            failure_streak: self.failure_streaks.get(&pair).map(|streak| {
                PolicyFailureStreakRecord {
                    failure_code: streak.failure_code.clone(),
                    failure_identity: streak.failure_identity.clone(),
                    class: streak.class,
                    count: streak.count,
                    escalation_streak: streak.escalation_streak,
                    disposition: streak.disposition,
                    retry_at_unix_ms: streak.retry_at_unix_ms,
                    decision_id: streak.decision_id.clone(),
                }
            }),
            retry_round: self
                .retry_rounds
                .get(&pair)
                .map(|round| PolicyRetryRoundRecord {
                    admission: round.admission.clone(),
                    until_unix_ms: round.until_unix_ms,
                    expected_duration_ms: round.expected_duration_ms,
                    opened: Some(round.opened),
                }),
            dispatch_clock: dispatch_clock.map(|admitted_at_unix_ms| PolicyDispatchClockRecord {
                admitted_at_unix_ms,
            }),
            condition_reruns: self.condition_reruns.get(&pair).cloned(),
            failure_settled: self.failure_settled.get(&pair).copied(),
            dispatches: Vec::new(),
        }
    }

    /// Workflow #422 LN-3 (register L7, L14): applies one value record; the record replaces its
    /// key's whole state. LN-4's start load runs it over the records the index names, in ledger
    /// order. The dispatch clock is the policy host's, not this state's. Workflow #422 LN-4
    /// (register §1.184 LN4-2): the record carries the Workflow #372 K5 orders, `failure_settled`
    /// and the retry round's `opened`, and the counters behind them go on after the largest one
    /// loaded; a record without a round's order (LN-3's) takes the next one in the order applied.
    pub(crate) fn apply_value_record(
        &mut self,
        record: &PolicyValueRecord,
    ) -> RuntimeHostResult<()> {
        match record {
            PolicyValueRecord::Control(state) => {
                let pair = (state.task_id.clone(), state.instance_id.clone());
                let of_pair =
                    |task: &String, instance: &String| *task == pair.0 && *instance == pair.1;
                self.task_daily
                    .retain(|(task, instance, _), _| !of_pair(task, instance));
                self.task_window
                    .retain(|(task, instance, _), _| !of_pair(task, instance));
                self.task_runtime
                    .retain(|(task, instance, _), _| !of_pair(task, instance));
                for day in &state.daily_counts {
                    self.task_daily
                        .insert((pair.0.clone(), pair.1.clone(), day.local_day), day.count);
                }
                match &state.active_window {
                    Some(window) => {
                        let key = (pair.0.clone(), pair.1.clone(), window.clone());
                        self.task_window.insert(key.clone(), state.window_counts);
                        self.task_runtime.insert(key, state.window_runtime_ms);
                        self.task_active_window.insert(pair.clone(), window.clone());
                    }
                    None => {
                        self.task_active_window.remove(&pair);
                    }
                }
                match &state.failure_streak {
                    Some(streak) => {
                        self.failure_streaks.insert(
                            pair.clone(),
                            FailureStreak {
                                failure_code: streak.failure_code.clone(),
                                failure_identity: streak.failure_identity.clone(),
                                class: streak.class,
                                count: streak.count,
                                escalation_streak: streak.escalation_streak,
                                disposition: streak.disposition,
                                retry_at_unix_ms: streak.retry_at_unix_ms,
                                decision_id: streak.decision_id.clone(),
                                paused_on: (streak.disposition
                                    == PolicyFailureDisposition::PausedTask)
                                    .then(|| {
                                        PausedPackages::of_identity(
                                            streak.failure_identity.as_deref(),
                                        )
                                    }),
                            },
                        );
                    }
                    None => {
                        self.failure_streaks.remove(&pair);
                    }
                }
                match state.failure_settled {
                    Some(settled) => {
                        self.settlements = self.settlements.max(settled);
                        self.failure_settled.insert(pair.clone(), settled);
                    }
                    None => {
                        self.failure_settled.remove(&pair);
                    }
                }
                // Workflow #386 SR-2b-1: the owed rerun as the pair's settlement wrote it.
                match &state.condition_reruns {
                    Some(reruns) => {
                        self.condition_reruns.insert(pair.clone(), reruns.clone());
                    }
                    None => {
                        self.condition_reruns.remove(&pair);
                    }
                }
                match &state.retry_round {
                    Some(round) => {
                        let opened = match round.opened {
                            Some(opened) => opened,
                            None => self.rounds_opened.checked_add(1).ok_or_else(|| {
                                fatal(
                                    HostCode::PolicyBudgetCounterOverflow,
                                    HostLocation::RecoverPolicyControl,
                                )
                            })?,
                        };
                        self.rounds_opened = self.rounds_opened.max(opened);
                        self.retry_rounds.insert(
                            pair,
                            RetryRound {
                                admission: round.admission.clone(),
                                until_unix_ms: round.until_unix_ms,
                                expected_duration_ms: round.expected_duration_ms,
                                declared_duration_ms: None,
                                opened,
                            },
                        );
                    }
                    None => {
                        self.retry_rounds.remove(&pair);
                    }
                }
            }
            PolicyValueRecord::Activity(state) => {
                let window = (
                    state.profile_id.clone(),
                    state.instance_id.clone(),
                    state.window_id.clone(),
                );
                self.activity_daily.insert(
                    (
                        state.profile_id.clone(),
                        state.instance_id.clone(),
                        state.local_day,
                    ),
                    state.daily_count,
                );
                self.activity_window
                    .insert(window.clone(), state.window_count);
                self.activity_runtime
                    .insert(window, state.window_runtime_ms);
                let cadence = (state.instance_id.clone(), state.profile_id.clone());
                match state.next_eligible_unix_ms {
                    Some(eligible) => {
                        self.next_activity_eligible.insert(cadence, eligible);
                    }
                    None => {
                        self.next_activity_eligible.remove(&cadence);
                    }
                }
                self.activity_active.insert(
                    (state.profile_id.clone(), state.instance_id.clone()),
                    (state.local_day, state.window_id.clone()),
                );
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn preview_admission(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        self.preview_admission_lifted(catalog, intent, now_unix_ms, None)
    }

    /// The live admission gate. A paused pair is admitted again once `lift` shows its
    /// suspension lifted (Workflow #336 L6, §12.7); without a view nothing is lifted.
    pub(crate) fn preview_admission_lifted(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
        lift: Option<&SuspensionLiftView<'_>>,
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        let result = self
            .preview_failure_disposition(catalog, intent, now_unix_ms, lift)
            .and_then(|()| self.preview_admission_at(catalog, intent, now_unix_ms));
        match result {
            Err(mut error) if is_availability_denial(&error) => {
                let mut rejection = error.policy_rejection();
                rejection.next_eligible_unix_ms =
                    self.next_admission_time(catalog, intent, now_unix_ms, lift)?;
                error.lifecycle.policy_rejection = Some(Box::new(rejection));
                Err(error)
            }
            result => result,
        }
    }

    /// Whether the pair's suspension is lifted under `lift` (§12.7), with a
    /// `lift_check_hash_unmeasured` warning for each package the check could not measure
    /// (Workflow #336 PKG-S3). Workflow #361 M6: a run the daemon's own end cut short
    /// (`policy_settlement_interrupted`) is no failure of the task, so its suspension does not
    /// outlive the restart that recorded it. Review P5: only the first such interruption in a
    /// row; a second one in a row (a task that may itself end the daemon) keeps the pair paused
    /// until its package changes, as before.
    fn suspension_lifted(
        failure: &FailureStreak,
        intent: &DispatchIntent,
        lift: Option<&SuspensionLiftView<'_>>,
    ) -> RuntimeHostResult<(bool, Vec<Outcome>)> {
        if interruption_lifted_by_restart(&failure.failure_code, failure.count) {
            return Ok((true, Vec::new()));
        }
        let (Some(view), Some(paused_on)) = (lift, failure.paused_on.as_ref()) else {
            return Ok((false, Vec::new()));
        };
        let verdict = view.lifted(&intent.procedure_ref, &failure.decision_id, paused_on)?;
        Ok((verdict.lifted.is_some(), verdict.unmeasured))
    }

    /// Workflow #372 K5: the key of the failure state `task_id` reads on `instance_id` under
    /// `catalog`: that of the id of its accounting group whose failure state a settlement set or
    /// cleared last, or its own when none was. A pause, a backoff or a streak of a continued id
    /// therefore follows the task that continues it, until that task settles itself.
    fn failure_key(
        &self,
        catalog: &CompiledCatalog,
        task_id: &str,
        instance_id: &str,
    ) -> (String, String) {
        catalog
            .accounting_ids(task_id)
            .into_iter()
            .filter_map(|id| {
                let key = (id.to_owned(), instance_id.to_owned());
                self.failure_settled
                    .get(&key)
                    .map(|settled| (*settled, key))
            })
            .max_by_key(|(settled, _)| *settled)
            .map_or_else(
                || (task_id.to_owned(), instance_id.to_owned()),
                |(_, key)| key,
            )
    }

    /// Workflow #372 K5: the failure streak `task_id` reads on `instance_id` (`failure_key`).
    fn group_failure(
        &self,
        catalog: &CompiledCatalog,
        task_id: &str,
        instance_id: &str,
    ) -> Option<&FailureStreak> {
        self.failure_streaks
            .get(&self.failure_key(catalog, task_id, instance_id))
    }

    /// Workflow #372 K5: the retry round `task_id` reads on `instance_id`: the latest one opened
    /// for an id of its accounting group.
    fn group_round(
        &self,
        catalog: &CompiledCatalog,
        task_id: &str,
        instance_id: &str,
    ) -> Option<&RetryRound> {
        catalog
            .accounting_ids(task_id)
            .into_iter()
            .filter_map(|id| {
                self.retry_rounds
                    .get(&(id.to_owned(), instance_id.to_owned()))
            })
            .max_by_key(|round| round.opened)
    }

    fn preview_failure_disposition(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
        lift: Option<&SuspensionLiftView<'_>>,
    ) -> RuntimeHostResult<()> {
        if let Some(failure) = self.group_failure(catalog, &intent.task_id, &intent.instance_id) {
            if failure.disposition == PolicyFailureDisposition::PausedTask {
                let (lifted, unmeasured) = Self::suspension_lifted(failure, intent, lift)?;
                if lifted {
                    return Ok(());
                }
                // A package the lift check could not measure keeps the pause and is reported
                // as a note of it (M-PKG D18).
                return Err(unmeasured.into_iter().fold(
                    request(
                        HostCode::PolicyTaskPaused,
                        HostLocation::ReservePolicyBudget,
                    ),
                    RuntimeHostError::with_note,
                ));
            }
            if failure
                .retry_at_unix_ms
                .is_some_and(|retry_at| now_unix_ms < retry_at)
            {
                return Err(request(
                    HostCode::PolicyRetryBackoffActive,
                    HostLocation::ReservePolicyBudget,
                ));
            }
        }
        Ok(())
    }

    fn preview_admission_at(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        let (task, profile) = task_and_profile(catalog, intent)?;
        let (local_day, window_id) = active_activity_window(profile, now_unix_ms)?;
        let cadence_key = (intent.instance_id.clone(), profile.id.clone());
        if self
            .next_activity_eligible
            .get(&cadence_key)
            .is_some_and(|eligible| now_unix_ms < *eligible)
        {
            return Err(request(
                HostCode::PolicyActivityIntervalActive,
                HostLocation::ReservePolicyBudget,
            ));
        }

        let seed = activity_seed(&intent.decision_id, &profile.id, &window_id);
        let interval_ms = sampled_interval(profile, seed)?;
        let next_eligible_unix_ms = now_unix_ms.checked_add(interval_ms).ok_or_else(|| {
            fatal(
                HostCode::PolicyActivityTimeOverflow,
                HostLocation::ReservePolicyBudget,
            )
        })?;

        // Workflow #372 K5: the task budgets count the task's accounting group, each entry
        // keeping its own id's runs.
        let accounting = catalog.accounting_ids(&intent.task_id);
        let activity_daily_key = (profile.id.clone(), intent.instance_id.clone(), local_day);
        let activity_window_key = (
            profile.id.clone(),
            intent.instance_id.clone(),
            window_id.clone(),
        );

        let budget = PolicyBudgetReceipt {
            task_daily_used: next_count(
                group_total(
                    &self.task_daily,
                    &accounting,
                    &intent.instance_id,
                    &local_day,
                    u32::checked_add,
                    HostLocation::ReservePolicyBudget,
                )?,
                task.loop_budget.daily_limit,
                PolicyBudgetDimension::TaskDaily,
            )?,
            task_daily_limit: task.loop_budget.daily_limit,
            task_window_used: next_count(
                group_total(
                    &self.task_window,
                    &accounting,
                    &intent.instance_id,
                    &window_id,
                    u32::checked_add,
                    HostLocation::ReservePolicyBudget,
                )?,
                task.loop_budget.window_iteration_limit,
                PolicyBudgetDimension::TaskWindow,
            )?,
            task_window_limit: task.loop_budget.window_iteration_limit,
            task_runtime_reserved_ms: next_runtime(
                group_total(
                    &self.task_runtime,
                    &accounting,
                    &intent.instance_id,
                    &window_id,
                    u64::checked_add,
                    HostLocation::ReservePolicyBudget,
                )?,
                intent.expected_duration_ms,
                task.loop_budget.max_runtime_ms,
                PolicyBudgetDimension::TaskRuntime,
            )?,
            task_runtime_limit_ms: task.loop_budget.max_runtime_ms,
            activity_daily_used: next_count(
                self.activity_daily
                    .get(&activity_daily_key)
                    .copied()
                    .unwrap_or(0),
                profile.daily_budget,
                PolicyBudgetDimension::ActivityDaily,
            )?,
            activity_daily_limit: profile.daily_budget,
            activity_window_used: next_count(
                self.activity_window
                    .get(&activity_window_key)
                    .copied()
                    .unwrap_or(0),
                profile.max_window_iterations,
                PolicyBudgetDimension::ActivityWindow,
            )?,
            activity_window_limit: profile.max_window_iterations,
            activity_runtime_reserved_ms: next_runtime(
                self.activity_runtime
                    .get(&activity_window_key)
                    .copied()
                    .unwrap_or(0),
                intent.expected_duration_ms,
                profile.session_max_ms,
                PolicyBudgetDimension::ActivityRuntime,
            )?,
            activity_runtime_limit_ms: profile.session_max_ms,
        };
        Ok(PolicyAdmissionRecord {
            activity: PolicyActivitySample {
                profile_id: profile.id.clone(),
                local_day,
                window_id,
                admitted_at_unix_ms: now_unix_ms,
                seed,
                interval_ms,
                next_eligible_unix_ms,
            },
            budget,
        })
    }

    fn next_admission_time(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
        lift: Option<&SuspensionLiftView<'_>>,
    ) -> RuntimeHostResult<Option<u64>> {
        let (_, profile) = task_and_profile(catalog, intent)?;
        let failure = self.group_failure(catalog, &intent.task_id, &intent.instance_id);
        if let Some(failure) = failure
            && failure.disposition == PolicyFailureDisposition::PausedTask
            && !Self::suspension_lifted(failure, intent, lift)?.0
        {
            return Ok(None);
        }
        let from = self
            .next_activity_eligible
            .get(&(intent.instance_id.clone(), profile.id.clone()))
            .copied()
            .unwrap_or(now_unix_ms)
            .max(now_unix_ms)
            .max(
                failure
                    .and_then(|failure| failure.retry_at_unix_ms)
                    .unwrap_or(now_unix_ms),
            );
        let mut boundaries = BTreeSet::from([from]);
        // Weekly windows repeat. Include one full week and the following day,
        // including overnight ends and full-day resets; test every boundary with
        // the very same admission predicate and counters used under the final lock.
        for window in &profile.windows {
            let offset = i128::from(window.utc_offset_minutes) * MILLIS_PER_MINUTE;
            let local_day = (i128::from(from) + offset).div_euclid(MILLIS_PER_DAY);
            for day in local_day..=local_day + 8 {
                for minute in [0, window.start_minute_of_day, window.end_minute_of_day] {
                    let boundary =
                        day * MILLIS_PER_DAY + i128::from(minute) * MILLIS_PER_MINUTE - offset;
                    if let Ok(boundary) = u64::try_from(boundary)
                        && boundary > now_unix_ms
                        && boundary >= from
                    {
                        boundaries.insert(boundary);
                    }
                }
            }
        }
        for boundary in boundaries.into_iter().filter(|time| *time > now_unix_ms) {
            match self.preview_admission_at(catalog, intent, boundary) {
                Ok(_) => return Ok(Some(boundary)),
                Err(error) if is_availability_denial(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    pub(crate) fn commit_admission(
        &mut self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        immediate_retry: bool,
    ) -> RuntimeHostResult<()> {
        // Replaying an accepted admission validates its original budget receipt.
        // Live failure disposition is checked by preview_admission under the owner lock.
        let expected =
            self.preview_admission_at(catalog, intent, admission.activity.admitted_at_unix_ms)?;
        if &expected != admission {
            return Err(fatal(
                HostCode::PolicyBudgetReceiptMismatch,
                HostLocation::CommitPolicyBudget,
            ));
        }
        if !immediate_retry {
            let (task, profile) = task_and_profile(catalog, intent)?;
            let window = activity_window_at(profile, admission.activity.admitted_at_unix_ms)
                .map_err(|_| {
                    fatal(
                        HostCode::PolicyActivityDayOverflow,
                        HostLocation::CommitPolicyBudget,
                    )
                })?
                .ok_or_else(|| {
                    fatal(
                        HostCode::PolicyActivityWindowClosed,
                        HostLocation::CommitPolicyBudget,
                    )
                })?;
            self.rounds_opened = self.rounds_opened.checked_add(1).ok_or_else(|| {
                fatal(
                    HostCode::PolicyBudgetCounterOverflow,
                    HostLocation::CommitPolicyBudget,
                )
            })?;
            self.retry_rounds.insert(
                (intent.task_id.clone(), intent.instance_id.clone()),
                RetryRound {
                    admission: admission.clone(),
                    until_unix_ms: window.until_unix_ms,
                    expected_duration_ms: intent.expected_duration_ms,
                    declared_duration_ms: Some(task.expected_duration_ms),
                    opened: self.rounds_opened,
                },
            );
        }
        let task_daily_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.local_day,
        );
        let task_window_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        let activity_daily_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.local_day,
        );
        let activity_window_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        // Workflow #372 K5: the receipt counts the task's accounting group; the task's own
        // entries keep only its own runs, so a later lookup never counts a continued id twice.
        let accounting = catalog.accounting_ids(&intent.task_id);
        let continued = accounting.get(1..).unwrap_or_default();
        let own = |used: u64, others: u64| {
            used.checked_sub(others).ok_or_else(|| {
                fatal(
                    HostCode::PolicyBudgetReceiptInvalid,
                    HostLocation::CommitPolicyBudget,
                )
            })
        };
        let own_count = |used: u32, others: u32| {
            own(u64::from(used), u64::from(others)).and_then(|count| {
                u32::try_from(count).map_err(|_| {
                    fatal(
                        HostCode::PolicyBudgetReceiptInvalid,
                        HostLocation::CommitPolicyBudget,
                    )
                })
            })
        };
        let task_daily_own = own_count(
            admission.budget.task_daily_used,
            group_total(
                &self.task_daily,
                continued,
                &intent.instance_id,
                &admission.activity.local_day,
                u32::checked_add,
                HostLocation::CommitPolicyBudget,
            )?,
        )?;
        let task_window_own = own_count(
            admission.budget.task_window_used,
            group_total(
                &self.task_window,
                continued,
                &intent.instance_id,
                &admission.activity.window_id,
                u32::checked_add,
                HostLocation::CommitPolicyBudget,
            )?,
        )?;
        let task_runtime_own = own(
            admission.budget.task_runtime_reserved_ms,
            group_total(
                &self.task_runtime,
                continued,
                &intent.instance_id,
                &admission.activity.window_id,
                u64::checked_add,
                HostLocation::CommitPolicyBudget,
            )?,
        )?;
        self.task_daily.insert(task_daily_key, task_daily_own);
        self.task_window
            .insert(task_window_key.clone(), task_window_own);
        self.task_runtime.insert(task_window_key, task_runtime_own);
        self.activity_daily
            .insert(activity_daily_key, admission.budget.activity_daily_used);
        self.activity_window.insert(
            activity_window_key.clone(),
            admission.budget.activity_window_used,
        );
        self.activity_runtime.insert(
            activity_window_key,
            admission.budget.activity_runtime_reserved_ms,
        );
        self.next_activity_eligible.insert(
            (
                intent.instance_id.clone(),
                admission.activity.profile_id.clone(),
            ),
            admission.activity.next_eligible_unix_ms,
        );
        // Workflow #422 LN-3: the window and the day the pair's and the activity's value records
        // count in.
        self.task_active_window.insert(
            (intent.task_id.clone(), intent.instance_id.clone()),
            admission.activity.window_id.clone(),
        );
        self.activity_active.insert(
            (
                admission.activity.profile_id.clone(),
                intent.instance_id.clone(),
            ),
            (
                admission.activity.local_day,
                admission.activity.window_id.clone(),
            ),
        );
        // Counts are spent on admission. Runtime reservations settle to actual usage first.
        self.end_exhausted_retry_rounds(catalog, &intent.instance_id, false)
    }

    #[cfg(test)]
    pub(crate) fn preview_execution(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        timing: PolicyExecutionTiming,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        self.preview_settlement(
            catalog,
            intent,
            admission,
            timing,
            input,
            perf_context,
            None,
        )
    }

    /// Test builds: [`Self::preview_paced_settlement`] of a run settled on its wall time.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn preview_settlement(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        timing: PolicyExecutionTiming,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
        condition: Option<PolicyConditionSettlement>,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        self.preview_paced_settlement(
            catalog,
            intent,
            admission,
            timing,
            None,
            input,
            perf_context,
            condition,
        )
    }

    /// The execution record a run settles to. Workflow #386 SR-2a (SCHED §4.2): `condition` is
    /// how a run that a condition ended settled. A replay passes its record's own member, as it
    /// passes `perf_context`, so the settlement never reads the configuration; the live
    /// settlement and the reconciliation from a run's terminal pass the one
    /// [`Self::decide_condition`] decided (Workflow #386 SR-2b-1).
    ///
    /// #385 P-k (PERF v1.4.2 §5.3, Tk-10): `budget_elapsed_ms` is the budget time of a run that
    /// had a factor segment above 1000. Such a run is compared with its runtime budget on its
    /// budget time, and a rewrite records the number compared; without one (`None`) the wall
    /// time `timing.runtime_ms` is compared. A replay passes its record's own member.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn preview_paced_settlement(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        timing: PolicyExecutionTiming,
        budget_elapsed_ms: Option<u64>,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
        condition: Option<PolicyConditionSettlement>,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let PolicyExecutionTiming {
            observed_at_unix_ms,
            runtime_ms,
        } = timing;
        let compared_ms = budget_elapsed_ms.unwrap_or(runtime_ms);
        let (task, _) = task_and_profile(catalog, intent)?;
        // Only a recoverable failure of a task that is not sensitive settles by a condition: its
        // record continues, and a sensitive or severe failure pauses (register S1).
        if condition.is_some()
            && (task.sensitive
                || !matches!(
                    input,
                    PolicyExecutionInput::Failed {
                        class: PolicyFailureClass::Recoverable,
                        ..
                    }
                ))
        {
            return Err(fatal(
                HostCode::PolicyExecutionRecordMismatch,
                HostLocation::ClassifyPolicyOutcome,
            ));
        }
        // SCHED §4.2 item 4: a run a condition ended is never re-judged as over its runtime
        // budget.
        let runtime_exceeded =
            condition.is_none() && runtime_budget_exceeded(intent, admission, compared_ms)?;
        // #385 P-k: the rewrite's runtime is the number compared.
        let runtime_ms = if runtime_exceeded {
            compared_ms
        } else {
            runtime_ms
        };
        let effective_input = if runtime_exceeded {
            PolicyExecutionInput::Failed {
                outcome: Outcome::new(actingcommand_contract::POLICY_RUNTIME_BUDGET_EXCEEDED),
                failure_identity: None,
                class: PolicyFailureClass::Severe,
            }
        } else {
            input.clone()
        };
        let outcome = match effective_input {
            PolicyExecutionInput::Succeeded => PolicyExecutionOutcome::Succeeded { runtime_ms },
            // SCHED §4.2 items 1 and 2: a run a condition ended adds to no failure streak and
            // escalates nothing. Its record keeps the pair's previous counters (0 when there are
            // none) and continues.
            PolicyExecutionInput::Failed {
                outcome: failure_outcome,
                failure_identity,
                class,
            } if condition.is_some() => {
                let previous = self.group_failure(catalog, &intent.task_id, &intent.instance_id);
                PolicyExecutionOutcome::Failed {
                    failure: PolicyFailureRecord {
                        outcome: failure_outcome,
                        failure_identity,
                        reported_success: false,
                        original_class: class,
                        effective_class: class,
                        consecutive_same_error: previous.map_or(0, |previous| previous.count),
                        escalation_streak: previous
                            .map_or(0, |previous| previous.escalation_streak),
                        performance_tax_exempt: class == PolicyFailureClass::Recoverable
                            && !task.sensitive
                            && perf_context.pressure_observed(),
                        retry_attempt: 0,
                        disposition: PolicyFailureDisposition::Continue,
                        retry_at_unix_ms: None,
                        runtime_ms,
                        sensitive: task.sensitive,
                        perf_context: Box::new(perf_context.clone()),
                        condition,
                    },
                }
            }
            PolicyExecutionInput::Failed {
                outcome: failure_outcome,
                failure_identity,
                class,
            } => {
                // Workflow #372 K5: a settlement counts on from the failure state of the task's
                // accounting group.
                let previous = self.group_failure(catalog, &intent.task_id, &intent.instance_id);
                // The same error: the same code, failure identity and class (never the whole
                // outcome, whose other values may differ between runs).
                let failure_code = scheduled_failure_code_str(&failure_outcome);
                let same_error = |previous: &FailureStreak| {
                    previous.failure_code == failure_code
                        && previous.failure_identity == failure_identity
                        && previous.class == class
                };
                let consecutive_same_error = match previous {
                    Some(previous) if same_error(previous) => {
                        previous.count.checked_add(1).ok_or_else(|| {
                            fatal(
                                HostCode::PolicyFailureCountOverflow,
                                HostLocation::ClassifyPolicyOutcome,
                            )
                        })?
                    }
                    _ => 1,
                };
                let performance_tax_exempt = class == PolicyFailureClass::Recoverable
                    && !task.sensitive
                    && perf_context.pressure_observed();
                let escalation_streak = match previous {
                    Some(previous) if same_error(previous) => {
                        if performance_tax_exempt {
                            previous.escalation_streak
                        } else {
                            previous.escalation_streak.checked_add(1).ok_or_else(|| {
                                fatal(
                                    HostCode::PolicyFailureCountOverflow,
                                    HostLocation::ClassifyPolicyOutcome,
                                )
                            })?
                        }
                    }
                    _ if performance_tax_exempt => 0,
                    _ => 1,
                };
                let effective_class = if class == PolicyFailureClass::Severe
                    || task.sensitive
                    || escalation_streak >= task.on_failure.escalation_threshold
                {
                    PolicyFailureClass::Severe
                } else {
                    PolicyFailureClass::Recoverable
                };
                let (disposition, retry_attempt, retry_at_unix_ms) =
                    if effective_class == PolicyFailureClass::Severe {
                        (PolicyFailureDisposition::PausedTask, 0, None)
                    } else if consecutive_same_error <= task.on_failure.retry_limit {
                        let backoff_ms = retry_backoff_ms(task, consecutive_same_error);
                        let retry_at =
                            observed_at_unix_ms.checked_add(backoff_ms).ok_or_else(|| {
                                fatal(
                                    HostCode::PolicyRetryTimeOverflow,
                                    HostLocation::ClassifyPolicyOutcome,
                                )
                            })?;
                        (
                            PolicyFailureDisposition::RetryScheduled,
                            consecutive_same_error,
                            Some(retry_at),
                        )
                    } else {
                        let disposition = match task.on_failure.action {
                            FailureAction::Continue => PolicyFailureDisposition::Continue,
                            FailureAction::Pause => PolicyFailureDisposition::PausedTask,
                        };
                        (disposition, 0, None)
                    };
                // The input's outcome and identity are recorded verbatim, so replay rebuilds
                // the same record from them.
                PolicyExecutionOutcome::Failed {
                    failure: PolicyFailureRecord {
                        outcome: failure_outcome,
                        failure_identity,
                        reported_success: runtime_exceeded
                            && matches!(input, PolicyExecutionInput::Succeeded),
                        original_class: class,
                        effective_class,
                        consecutive_same_error,
                        escalation_streak,
                        performance_tax_exempt,
                        retry_attempt,
                        disposition,
                        retry_at_unix_ms,
                        runtime_ms,
                        sensitive: task.sensitive,
                        perf_context: Box::new(perf_context.clone()),
                        condition: None,
                    },
                }
            }
        };
        Ok(PolicyExecutionEventData {
            decision_id: intent.decision_id.clone(),
            task_id: intent.task_id.clone(),
            instance_id: intent.instance_id.clone(),
            observed_at_unix_ms,
            outcome,
        })
    }

    /// Workflow #386 SR-2b-2: the owed reruns as the pairs' settlements left them (register L7),
    /// by (task, instance): the evaluation's input, its expiry and `actingd suspended` read them.
    pub(crate) fn condition_reruns(&self) -> &BTreeMap<(String, String), PolicyConditionReruns> {
        &self.condition_reruns
    }

    /// Workflow #386 SR-2b-1 (`contracts/policy-suspension.md`, "Decided once"): whether the
    /// first settlement of `intent`'s run, failed as `input` says, ends it by a condition. Only
    /// a recoverable failure of a task that is not sensitive, whose code
    /// ([`scheduled_failure_code_str`]) `rule` lists, does. Its `count` is the pair's
    /// consecutive condition terminations in this occurrence: one more than the owed rerun's
    /// when that one expires at the same window end, else 1. `expires_at_unix_ms` is the end of
    /// the activity window the run was admitted in (the pair's bookkeeping window). A count
    /// above `max_per_occurrence` is [`ConditionDecision::Exhausted`]. The decision is written
    /// into the run's record; a replay settles by the record and never calls this.
    pub(crate) fn decide_condition(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        input: &PolicyExecutionInput,
        rule: &ConditionRule,
    ) -> RuntimeHostResult<ConditionDecision> {
        let PolicyExecutionInput::Failed {
            outcome,
            class: PolicyFailureClass::Recoverable,
            ..
        } = input
        else {
            return Ok(ConditionDecision::Ordinary);
        };
        let (task, profile) = task_and_profile(catalog, intent)?;
        if task.sensitive
            || !rule
                .condition_codes
                .contains(scheduled_failure_code_str(outcome).as_str())
        {
            return Ok(ConditionDecision::Ordinary);
        }
        // The admission was committed inside this window (`commit_admission`), under the same
        // catalog generation, so it is found again.
        let window = activity_window_at(profile, admission.activity.admitted_at_unix_ms)
            .map_err(|_| {
                fatal(
                    HostCode::PolicyActivityDayOverflow,
                    HostLocation::ClassifyPolicyOutcome,
                )
            })?
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyActivityWindowClosed,
                    HostLocation::ClassifyPolicyOutcome,
                )
            })?;
        let pair = (intent.task_id.clone(), intent.instance_id.clone());
        let count = match self.condition_reruns.get(&pair) {
            Some(owed) if owed.expires_at_unix_ms == window.until_unix_ms => {
                owed.count.checked_add(1).ok_or_else(|| {
                    fatal(
                        HostCode::PolicyFailureCountOverflow,
                        HostLocation::ClassifyPolicyOutcome,
                    )
                })?
            }
            _ => 1,
        };
        if count > rule.max_per_occurrence {
            return Ok(ConditionDecision::Exhausted {
                count,
                limit: rule.max_per_occurrence,
                window_end_unix_ms: window.until_unix_ms,
            });
        }
        Ok(ConditionDecision::Condition(PolicyConditionSettlement {
            refunded: rule.refund_counts,
            count,
            expires_at_unix_ms: window.until_unix_ms,
        }))
    }

    /// Test builds: [`Self::commit_paced_execution`] of a record settled on its wall time.
    #[cfg(test)]
    pub(crate) fn commit_execution(
        &mut self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        data: &PolicyExecutionEventData,
    ) -> RuntimeHostResult<()> {
        self.commit_paced_execution(catalog, intent, admission, data, None)
    }

    /// Commits the execution record `data`, checked against the settlement it must be. #385 P-k
    /// (register P6): `budget_elapsed_ms` is the budget time the record carries, which its
    /// settlement compared; `None` for a record settled on its wall time.
    pub(crate) fn commit_paced_execution(
        &mut self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        data: &PolicyExecutionEventData,
        budget_elapsed_ms: Option<u64>,
    ) -> RuntimeHostResult<()> {
        let input = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => PolicyExecutionInput::Succeeded,
            PolicyExecutionOutcome::Failed { failure } if failure.reported_success => {
                PolicyExecutionInput::Succeeded
            }
            PolicyExecutionOutcome::Failed { failure } => PolicyExecutionInput::Failed {
                outcome: failure.outcome.clone(),
                failure_identity: failure.failure_identity.clone(),
                class: failure.original_class,
            },
        };
        let perf_context = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => PerformanceContext::unavailable(1),
            PolicyExecutionOutcome::Failed { failure } => failure.perf_context.as_ref().clone(),
        };
        let runtime_ms = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => *runtime_ms,
            PolicyExecutionOutcome::Failed { failure } => failure.runtime_ms,
        };
        // Workflow #386 SR-2a (SCHED §4.2): the condition comes from the record, never from the
        // configuration, so a replay settles as the record did.
        let condition = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => None,
            PolicyExecutionOutcome::Failed { failure } => failure.condition,
        };
        let expected = self.preview_paced_settlement(
            catalog,
            intent,
            admission,
            PolicyExecutionTiming {
                observed_at_unix_ms: data.observed_at_unix_ms,
                runtime_ms,
            },
            budget_elapsed_ms,
            &input,
            &perf_context,
            condition,
        )?;
        if &expected != data {
            return Err(fatal(
                HostCode::PolicyExecutionRecordMismatch,
                HostLocation::CommitPolicyOutcome,
            ));
        }
        // SCHED §4.2 item 4: the runtime of a run a condition ended is not counted; the windows
        // keep what they held before its reservation.
        let counted_runtime_ms = if condition.is_some() { 0 } else { runtime_ms };
        let (task_runtime_used_ms, activity_runtime_used_ms) =
            actual_runtime_totals(intent, admission, counted_runtime_ms)?;
        // Workflow #372 K5: the receipt's runtime counts the task's accounting group; the task's
        // own entry keeps only its own runtime.
        let accounting = catalog.accounting_ids(&intent.task_id);
        let continued_runtime_ms = group_total(
            &self.task_runtime,
            accounting.get(1..).unwrap_or_default(),
            &intent.instance_id,
            &admission.activity.window_id,
            u64::checked_add,
            HostLocation::CommitPolicyOutcome,
        )?;
        let task_runtime_own_ms = task_runtime_used_ms
            .checked_sub(continued_runtime_ms)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyBudgetReceiptInvalid,
                    HostLocation::CommitPolicyOutcome,
                )
            })?;
        let task_window_key = (
            intent.task_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        let activity_window_key = (
            admission.activity.profile_id.clone(),
            intent.instance_id.clone(),
            admission.activity.window_id.clone(),
        );
        self.task_runtime
            .insert(task_window_key, task_runtime_own_ms);
        self.activity_runtime
            .insert(activity_window_key, activity_runtime_used_ms);
        let key = (intent.task_id.clone(), intent.instance_id.clone());
        // Workflow #372 K5: a success or a failure no condition settled sets the pair's failure
        // state, so it becomes the one its accounting group reads.
        if condition.is_none() {
            self.settlements = self.settlements.checked_add(1).ok_or_else(|| {
                fatal(
                    HostCode::PolicyFailureCountOverflow,
                    HostLocation::CommitPolicyOutcome,
                )
            })?;
            self.failure_settled.insert(key.clone(), self.settlements);
        }
        // Workflow #386 SR-2b-1 (register L7): a run a condition ended owes its pair a rerun, by
        // the record's own `condition`, so a replay rebuilds it; any other settlement of the
        // pair (a success, or an ordinary failure, the exhausted one included) ends the pair's
        // condition terminations and owes none.
        let owed = match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => None,
            PolicyExecutionOutcome::Failed { failure } => {
                failure.condition.map(|condition| PolicyConditionReruns {
                    failure_code: scheduled_failure_code_str(&failure.outcome),
                    count: condition.count,
                    expires_at_unix_ms: condition.expires_at_unix_ms,
                })
            }
        };
        match owed {
            Some(owed) => {
                self.condition_reruns.insert(key.clone(), owed);
            }
            None => {
                self.condition_reruns.remove(&key);
            }
        }
        match &data.outcome {
            PolicyExecutionOutcome::Succeeded { .. } => {
                self.failure_streaks.remove(&key);
                self.retry_rounds.remove(&key);
            }
            // SCHED §4.2 items 1 and 3: `failure_streaks` keeps the pair's latest real failure
            // (the identity a suspension lift compares), and a refunded run gives back the four
            // counts its admission took.
            PolicyExecutionOutcome::Failed { failure } if failure.condition.is_some() => {
                if failure
                    .condition
                    .is_some_and(|condition| condition.refunded)
                {
                    self.refund_admission_counts(intent, admission)?;
                }
            }
            PolicyExecutionOutcome::Failed { failure } => {
                // Live settlement and replay both pass here, so a restarted host rebuilds the
                // same suspension from the execution record (§12.7; Workflow #336 PKG-S3: the
                // hashes come from its failure identity, never from the dispatch).
                let paused_on = (failure.disposition == PolicyFailureDisposition::PausedTask)
                    .then(|| PausedPackages::of_identity(failure.failure_identity.as_deref()));
                self.failure_streaks.insert(
                    key,
                    FailureStreak {
                        failure_code: scheduled_failure_code_str(&failure.outcome),
                        failure_identity: failure.failure_identity.clone(),
                        class: failure.original_class,
                        count: failure.consecutive_same_error,
                        escalation_streak: failure.escalation_streak,
                        disposition: failure.disposition,
                        retry_at_unix_ms: failure.retry_at_unix_ms,
                        decision_id: data.decision_id.clone(),
                        paused_on,
                    },
                );
            }
        }
        self.end_exhausted_retry_rounds(catalog, &intent.instance_id, true)
    }

    /// Workflow #336 L6: the failure identity (if it has one) and the dispatch of a pair's
    /// latest failed execution, while no success has followed it.
    pub(crate) fn latest_failure(
        &self,
        task_id: &str,
        instance_id: &str,
    ) -> Option<(Option<&str>, &str)> {
        self.failure_streaks
            .get(&(task_id.to_owned(), instance_id.to_owned()))
            .map(|failure| {
                (
                    failure.failure_identity.as_deref(),
                    failure.decision_id.as_str(),
                )
            })
    }

    /// Workflow #372 K5: `latest_failure` of the failure state `task_id` reads on `instance_id`
    /// under `catalog` (that of its accounting group), so a new failure's identity is compared
    /// with the failure its streak counts on from.
    pub(crate) fn group_latest_failure(
        &self,
        catalog: &CompiledCatalog,
        task_id: &str,
        instance_id: &str,
    ) -> Option<(Option<&str>, &str)> {
        self.group_failure(catalog, task_id, instance_id)
            .map(|failure| {
                (
                    failure.failure_identity.as_deref(),
                    failure.decision_id.as_str(),
                )
            })
    }

    /// Workflow #384 D2-S4: the dispatches of `task_id` on `instance_id` admitted in the activity
    /// day `local_day` (the day the task daily budget counts).
    pub(crate) fn task_daily_count(&self, task_id: &str, instance_id: &str, local_day: i64) -> u32 {
        self.task_daily
            .get(&(task_id.to_owned(), instance_id.to_owned(), local_day))
            .copied()
            .unwrap_or(0)
    }

    /// Workflow #384 D2-S4: the registered code of the pair's latest failure while no success
    /// has followed it.
    pub(crate) fn latest_failure_code(&self, task_id: &str, instance_id: &str) -> Option<Code> {
        self.failure_streaks
            .get(&(task_id.to_owned(), instance_id.to_owned()))
            .and_then(|failure| failure.failure_code.registered())
    }

    /// R22 eligibility is bounded by the original normal admission, including after replay.
    /// Backoff and all remaining admission gates are checked by the ordinary predicate.
    pub(crate) fn immediate_retry_pairs(
        &self,
        catalog: &CompiledCatalog,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<BTreeSet<(String, String)>> {
        let mut pairs = BTreeSet::new();
        for (pair, failure) in &self.failure_streaks {
            if failure.disposition != PolicyFailureDisposition::RetryScheduled
                || failure
                    .failure_identity
                    .as_deref()
                    .and_then(FailureIdentity::parse)
                    .is_none()
            {
                continue;
            }
            // Workflow #372 K5: the entry stands for the task whose accounting group holds its
            // id, and only while it is that task's failure state; the round is that group's.
            let task_id = accounting_owner(catalog, &pair.0, &pair.1);
            if self.failure_key(catalog, task_id, &pair.1) != *pair {
                continue;
            }
            let Some(round) = self.group_round(catalog, task_id, &pair.1) else {
                continue;
            };
            let origin = &round.admission.activity;
            if now_unix_ms < origin.admitted_at_unix_ms || now_unix_ms >= round.until_unix_ms {
                continue;
            }
            // SCH-M ED-2: the task keeps the declared duration the round opened under.
            if !catalog.catalog().tasks.tasks.iter().any(|task| {
                task.id == task_id
                    && task.expected_duration_ms
                        == round
                            .declared_duration_ms
                            .unwrap_or(round.expected_duration_ms)
            }) {
                continue;
            }
            let Some(profile) = catalog
                .catalog()
                .activity
                .profiles
                .iter()
                .find(|profile| profile.id == origin.profile_id)
            else {
                continue;
            };
            let window = activity_window_at(profile, now_unix_ms).map_err(|_| {
                fatal(
                    HostCode::PolicyActivityDayOverflow,
                    HostLocation::ResolveRetryRound,
                )
            })?;
            if window.is_some_and(|window| {
                window.local_day == origin.local_day && window.window_id == origin.window_id
            }) && self.retry_round_has_budget(catalog, task_id, &pair.1, round, true)?
            {
                pairs.insert((task_id.to_owned(), pair.1.clone()));
            }
        }
        Ok(pairs)
    }

    /// Whether the round's limits leave room for one more run of `task_id` on `instance_id`.
    /// Workflow #372 K5: the task counts are those of its accounting group.
    fn retry_round_has_budget(
        &self,
        catalog: &CompiledCatalog,
        task_id: &str,
        instance_id: &str,
        round: &RetryRound,
        include_runtime: bool,
    ) -> RuntimeHostResult<bool> {
        let origin = &round.admission.activity;
        let limits = &round.admission.budget;
        let accounting = catalog.accounting_ids(task_id);
        let task_daily = group_total(
            &self.task_daily,
            &accounting,
            instance_id,
            &origin.local_day,
            u32::checked_add,
            HostLocation::ResolveRetryRound,
        )?;
        let task_window = group_total(
            &self.task_window,
            &accounting,
            instance_id,
            &origin.window_id,
            u32::checked_add,
            HostLocation::ResolveRetryRound,
        )?;
        let task_runtime = group_total(
            &self.task_runtime,
            &accounting,
            instance_id,
            &origin.window_id,
            u64::checked_add,
            HostLocation::ResolveRetryRound,
        )?;
        let activity_daily = (
            origin.profile_id.clone(),
            instance_id.to_owned(),
            origin.local_day,
        );
        let activity_window = (
            origin.profile_id.clone(),
            instance_id.to_owned(),
            origin.window_id.clone(),
        );
        Ok(task_daily < limits.task_daily_limit
            && task_window < limits.task_window_limit
            && self
                .activity_daily
                .get(&activity_daily)
                .copied()
                .unwrap_or(0)
                < limits.activity_daily_limit
            && self
                .activity_window
                .get(&activity_window)
                .copied()
                .unwrap_or(0)
                < limits.activity_window_limit
            && (!include_runtime
                || (task_runtime
                    .checked_add(round.expected_duration_ms)
                    .is_some_and(|next| next <= limits.task_runtime_limit_ms)
                    && self
                        .activity_runtime
                        .get(&activity_window)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(round.expected_duration_ms)
                        .is_some_and(|next| next <= limits.activity_runtime_limit_ms))))
    }

    pub(crate) fn validate_immediate_retry(
        &self,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let pair = (intent.task_id.clone(), intent.instance_id.clone());
        if !self
            .immediate_retry_pairs(catalog, now_unix_ms)?
            .contains(&pair)
            || self
                .group_round(catalog, &intent.task_id, &intent.instance_id)
                .is_none_or(|round| {
                    round.admission.activity.profile_id != intent.prerequisites.activity_profile_id
                        || intent.expected_duration_ms != round.expected_duration_ms
                })
        {
            return Err(request(
                HostCode::PolicyRetryRoundEnded,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        Ok(())
    }

    /// Workflow #386 SR-2a (SCHED §4.2 item 3): gives back the task daily, task window,
    /// activity daily and activity window counts `admission` took.
    fn refund_admission_counts(
        &mut self,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
    ) -> RuntimeHostResult<()> {
        let activity = &admission.activity;
        refund_count(
            &mut self.task_daily,
            (
                intent.task_id.clone(),
                intent.instance_id.clone(),
                activity.local_day,
            ),
        )?;
        refund_count(
            &mut self.task_window,
            (
                intent.task_id.clone(),
                intent.instance_id.clone(),
                activity.window_id.clone(),
            ),
        )?;
        refund_count(
            &mut self.activity_daily,
            (
                activity.profile_id.clone(),
                intent.instance_id.clone(),
                activity.local_day,
            ),
        )?;
        refund_count(
            &mut self.activity_window,
            (
                activity.profile_id.clone(),
                intent.instance_id.clone(),
                activity.window_id.clone(),
            ),
        )
    }

    fn end_exhausted_retry_rounds(
        &mut self,
        catalog: &CompiledCatalog,
        instance_id: &str,
        include_runtime: bool,
    ) -> RuntimeHostResult<()> {
        let mut ended = Vec::new();
        for (pair, round) in &self.retry_rounds {
            if pair.1 == instance_id
                && !self.retry_round_has_budget(
                    catalog,
                    accounting_owner(catalog, &pair.0, &pair.1),
                    &pair.1,
                    round,
                    include_runtime,
                )?
            {
                ended.push(pair.clone());
            }
        }
        for pair in ended {
            self.retry_rounds.remove(&pair);
        }
        Ok(())
    }
}

/// Workflow #372 K5: the task whose accounting group holds `task_id` on `instance_id` under
/// `catalog`: the task that continues it there, else `task_id` itself. ROUT-L RL-1b: read per
/// instance, as two instances of one game and server each continue the file's id with their own
/// copy.
fn accounting_owner<'a>(
    catalog: &'a CompiledCatalog,
    task_id: &'a str,
    instance_id: &str,
) -> &'a str {
    catalog
        .continued_by_on(task_id, instance_id)
        .unwrap_or(task_id)
}

/// Workflow #372 K5: the sum of the entries of `ids` on `instance_id` for one day or window
/// (`dimension`) of a task budget map. An overflow is fatal at `location`.
fn group_total<D: Ord + Clone, V: Copy + Default>(
    entries: &BTreeMap<(String, String, D), V>,
    ids: &[&str],
    instance_id: &str,
    dimension: &D,
    add: fn(V, V) -> Option<V>,
    location: HostLocation,
) -> RuntimeHostResult<V> {
    ids.iter().try_fold(V::default(), |total, id| {
        let entry = entries
            .get(&((*id).to_owned(), instance_id.to_owned(), dimension.clone()))
            .copied()
            .unwrap_or_default();
        add(total, entry).ok_or_else(|| fatal(HostCode::PolicyBudgetCounterOverflow, location))
    })
}

fn task_and_profile<'a>(
    catalog: &'a CompiledCatalog,
    intent: &DispatchIntent,
) -> RuntimeHostResult<(&'a TaskSpec, &'a ActivityProfile)> {
    // Workflow #384 D2-S4: a fact refresh dispatch runs under the Runtime-owned task spec.
    let task = catalog.dispatch_task(&intent.task_id).ok_or_else(|| {
        request(
            HostCode::PolicyTaskMissing,
            HostLocation::ResolvePolicyControl,
        )
    })?;
    let profile = catalog
        .catalog()
        .activity
        .profiles
        .iter()
        .find(|profile| profile.id == intent.prerequisites.activity_profile_id)
        .ok_or_else(|| {
            request(
                HostCode::PolicyActivityProfileMissing,
                HostLocation::ResolvePolicyControl,
            )
        })?;
    Ok((task, profile))
}

pub(crate) fn active_activity_window(
    profile: &ActivityProfile,
    now_unix_ms: u64,
) -> RuntimeHostResult<(i64, String)> {
    activity_window_at(profile, now_unix_ms)
        .map_err(|_| {
            fatal(
                HostCode::PolicyActivityDayOverflow,
                HostLocation::ResolveActivityWindow,
            )
        })?
        .map(|window| (window.local_day, window.window_id))
        .ok_or_else(|| {
            request(
                HostCode::PolicyActivityWindowClosed,
                HostLocation::ReservePolicyBudget,
            )
        })
}

fn activity_seed(decision_id: &str, profile_id: &str, window_id: &str) -> u64 {
    let digest = Sha256::digest(format!("{decision_id}\0{profile_id}\0{window_id}").as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

fn sampled_interval(profile: &ActivityProfile, seed: u64) -> RuntimeHostResult<u64> {
    let width = profile
        .maximum_interval_ms
        .checked_sub(profile.minimum_interval_ms)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyActivityIntervalInvalid,
                HostLocation::SampleActivityInterval,
            )
        })?;
    Ok(profile.minimum_interval_ms + seed % width)
}

pub(crate) fn is_availability_denial(error: &RuntimeHostError) -> bool {
    !error.is_fatal()
        && spelled(
            error.code(),
            &[
                HostCode::PolicyBudgetExhausted.code(),
                HostCode::PolicyActivityIntervalActive.code(),
                HostCode::PolicyActivityWindowClosed.code(),
                HostCode::PolicyRetryBackoffActive.code(),
                HostCode::PolicyTaskPaused.code(),
            ],
        )
}

fn budget_exhausted(
    dimension: PolicyBudgetDimension,
    used: u64,
    requested: u64,
    limit: u64,
) -> RuntimeHostError {
    let mut error = request(
        HostCode::PolicyBudgetExhausted,
        HostLocation::ReservePolicyBudget,
    );
    let mut rejection = error.policy_rejection();
    rejection.budget = Some(PolicyBudgetDenial {
        dimension,
        used,
        requested,
        limit,
    });
    error.lifecycle.policy_rejection = Some(Box::new(rejection));
    error
}

fn next_count(
    current: u32,
    limit: u32,
    dimension: PolicyBudgetDimension,
) -> RuntimeHostResult<u32> {
    let next = current.checked_add(1).ok_or_else(|| {
        fatal(
            HostCode::PolicyBudgetCounterOverflow,
            HostLocation::ReservePolicyBudget,
        )
    })?;
    if next > limit {
        return Err(budget_exhausted(
            dimension,
            u64::from(current),
            1,
            u64::from(limit),
        ));
    }
    Ok(next)
}

/// Takes back one count an admission spent. The admission put at least one there, so a count
/// that is missing or zero means the state lost it.
fn refund_count<K: Ord>(counts: &mut BTreeMap<K, u32>, key: K) -> RuntimeHostResult<()> {
    let count = counts
        .get_mut(&key)
        .filter(|count| **count > 0)
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyBudgetCounterOverflow,
                HostLocation::CommitPolicyOutcome,
            )
        })?;
    *count -= 1;
    Ok(())
}

fn next_runtime(
    current: u64,
    reservation: u64,
    limit: u64,
    dimension: PolicyBudgetDimension,
) -> RuntimeHostResult<u64> {
    let next = current.checked_add(reservation).ok_or_else(|| {
        fatal(
            HostCode::PolicyBudgetCounterOverflow,
            HostLocation::ReservePolicyBudget,
        )
    })?;
    if next > limit {
        return Err(budget_exhausted(dimension, current, reservation, limit));
    }
    Ok(next)
}

/// Workflow #369 E3: whether a run of `runtime_ms` exceeded the admitted runtime budget, by
/// the one rule the host's recovery check and the ledger also use
/// (`PolicyBudgetReceipt::runtime_exceeded`).
pub(crate) fn runtime_budget_exceeded(
    intent: &DispatchIntent,
    admission: &PolicyAdmissionRecord,
    runtime_ms: u64,
) -> RuntimeHostResult<bool> {
    admission
        .budget
        .runtime_exceeded(intent.expected_duration_ms, runtime_ms)
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyBudgetReceiptInvalid,
                HostLocation::ClassifyPolicyOutcome,
            )
        })
}

fn actual_runtime_totals(
    intent: &DispatchIntent,
    admission: &PolicyAdmissionRecord,
    runtime_ms: u64,
) -> RuntimeHostResult<(u64, u64)> {
    let task_before = admission
        .budget
        .task_runtime_reserved_ms
        .checked_sub(intent.expected_duration_ms)
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyBudgetReceiptInvalid,
                HostLocation::ClassifyPolicyOutcome,
            )
        })?;
    let activity_before = admission
        .budget
        .activity_runtime_reserved_ms
        .checked_sub(intent.expected_duration_ms)
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyBudgetReceiptInvalid,
                HostLocation::ClassifyPolicyOutcome,
            )
        })?;
    let task_total = task_before.checked_add(runtime_ms).ok_or_else(|| {
        fatal(
            HostCode::PolicyBudgetCounterOverflow,
            HostLocation::ClassifyPolicyOutcome,
        )
    })?;
    let activity_total = activity_before.checked_add(runtime_ms).ok_or_else(|| {
        fatal(
            HostCode::PolicyBudgetCounterOverflow,
            HostLocation::ClassifyPolicyOutcome,
        )
    })?;
    Ok((task_total, activity_total))
}

fn retry_backoff_ms(task: &TaskSpec, attempt: u16) -> u64 {
    let shift = u32::from(attempt.saturating_sub(1)).min(63);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    task.on_failure
        .retry_backoff_ms
        .max(1)
        .saturating_mul(multiplier)
        .min(task.next_run_clamp_ms)
}

fn request(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        PerformanceMonitorHealth, PerformancePressureKind, PerformancePressureRecord,
        PerformancePressureSeverity, PerformancePressureValue, PolicyExecutionOutcome,
        PolicyFailureDisposition,
    };
    use actingcommand_execution_kernel::ExecutionKernelCode;
    use actingcommand_policy::{
        CatalogDocumentSource, CatalogSources, DispatchPrerequisites, LoadProfile, compile_catalog,
    };

    const NOW: u64 = 1_699_963_200_000;

    fn sources() -> CatalogSources {
        CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://fixture/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://fixture/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://fixture/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://fixture/timeline.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                    .to_vec(),
            ),
            selection: None,
        }
    }

    fn catalog_with(
        task_mutator: impl FnOnce(&mut serde_json::Value),
        activity_mutator: impl FnOnce(&mut serde_json::Value),
    ) -> CompiledCatalog {
        let mut sources = sources();
        let mut tasks: serde_json::Value =
            serde_json::from_slice(&sources.tasks.bytes).expect("tasks JSON");
        task_mutator(&mut tasks);
        sources.tasks.bytes = serde_json::to_vec(&tasks).expect("tasks bytes");
        let mut activity: serde_json::Value =
            serde_json::from_slice(&sources.activity.bytes).expect("activity JSON");
        activity_mutator(&mut activity);
        sources.activity.bytes = serde_json::to_vec(&activity).expect("activity bytes");
        compile_catalog(&sources).expect("compiled catalog")
    }

    fn catalog() -> CompiledCatalog {
        catalog_with(|_| {}, |_| {})
    }

    fn unavailable(at_unix_ms: u64) -> PerformanceContext {
        PerformanceContext::unavailable(at_unix_ms)
    }

    fn pressured(at_unix_ms: u64) -> PerformanceContext {
        PerformanceContext {
            window_start_unix_ms: at_unix_ms.saturating_sub(30_000).max(1),
            window_end_unix_ms: at_unix_ms,
            health: PerformanceMonitorHealth::Healthy,
            sample_count: 1,
            unavailable_metrics: Vec::new(),
            pressures: vec![PerformancePressureRecord {
                kind: PerformancePressureKind::Cpu,
                severity: PerformancePressureSeverity::High,
                started_at_unix_ms: at_unix_ms.saturating_sub(1).max(1),
                last_observed_at_unix_ms: at_unix_ms,
                peak: PerformancePressureValue::Utilization {
                    basis_points: 9_500,
                },
            }],
            max_cpu_basis_points: Some(9_500),
            max_ram_basis_points: Some(4_000),
            disk_queue_depth_p95_milli: None,
            disk_latency_p95_micros: None,
            max_gpu_basis_points: None,
            max_frame_gap_ms: None,
            max_capture_latency_ms: None,
            max_recognition_latency_ms: None,
            max_action_effect_latency_ms: None,
            max_touch_response_us: None,
            max_capture_acquire_us: None,
            related_event_ids: Vec::new(),
        }
    }

    fn intent(catalog: &CompiledCatalog, suffix: u64) -> DispatchIntent {
        let task = &catalog.catalog().tasks.tasks[0];
        DispatchIntent {
            decision_id: format!("decision:fixture-{suffix}"),
            task_id: task.id.clone(),
            instance_id: "fixture-instance-a".to_owned(),
            operation_id: task.entrypoint.operation_id.clone(),
            procedure_ref: task.procedure_ref.clone(),
            procedure_binding_digest: Some(format!("sha256:{}", "b".repeat(64))),
            catalog_hash: catalog.catalog_hash().to_owned(),
            catalog_version: catalog.catalog().tasks.catalog.catalog_version,
            input_ledger_position: 1,
            fact_snapshot_id: "snapshot:fixture-a".to_owned(),
            approval_refs: catalog.catalog().tasks.catalog.approval_refs.clone(),
            reason_chain_id: format!("reason:fixture-{suffix}"),
            expected_duration_ms: task.expected_duration_ms,
            yield_points: task.yield_points.clone(),
            load_profile: LoadProfile::Light,
            prerequisites: DispatchPrerequisites {
                fencing_required: true,
                evaluated_at_unix_ms: NOW,
                facts_fresh_until_unix_ms: None,
                activity_profile_id: "fixture-activity-a".to_owned(),
                daily_limit: task.loop_budget.daily_limit,
                window_iteration_limit: task.loop_budget.window_iteration_limit,
                max_runtime_ms: task.loop_budget.max_runtime_ms,
                urgency_milli: 0,
            },
        }
    }

    #[test]
    fn recoverable_failures_back_off_then_escalate_same_error() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let mut recovered = PolicyControlState::default();
        let mut now = NOW;
        for attempt in 1_u16..=3 {
            let intent = intent(&catalog, u64::from(attempt));
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 100,
                        runtime_ms: 100,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                        failure_identity: None,
                        class: PolicyFailureClass::Recoverable,
                    },
                    &unavailable(now + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            if attempt < 3 {
                assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
                assert_eq!(
                    failure.disposition,
                    PolicyFailureDisposition::RetryScheduled
                );
                assert_eq!(failure.retry_attempt, attempt);
                assert!(failure.retry_at_unix_ms.is_some());
            } else {
                assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
                assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
                assert_eq!(failure.retry_attempt, 0);
                assert!(failure.retry_at_unix_ms.is_none());
            }
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
            recovered
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("replay the recorded failure");
            for control in [&state, &recovered] {
                let error = control
                    .preview_admission(&catalog, &intent, data.observed_at_unix_ms)
                    .expect_err("recorded failure must control admission");
                if let Some(retry_at) = failure.retry_at_unix_ms {
                    assert_eq!(error.code(), "policy_retry_backoff_active");
                    assert_eq!(
                        error.policy_rejection().next_eligible_unix_ms,
                        Some(retry_at)
                    );
                    assert!(
                        control
                            .preview_admission(&catalog, &intent, retry_at)
                            .is_ok()
                    );
                } else {
                    assert_eq!(error.code(), "policy_task_paused");
                    assert_eq!(error.policy_rejection().next_eligible_unix_ms, None);
                    assert_eq!(
                        control
                            .preview_admission(&catalog, &intent, NOW + 86_400_000)
                            .expect_err("pause survives a day boundary")
                            .code(),
                        "policy_task_paused"
                    );
                }
                let mut other_instance = intent.clone();
                other_instance.instance_id = "fixture-instance-b".to_owned();
                assert!(
                    control
                        .preview_admission(&catalog, &other_instance, now)
                        .is_ok()
                );
            }
            now = failure.retry_at_unix_ms.unwrap_or(now + 100);
        }
    }

    #[test]
    fn performance_pressure_exempts_escalation_but_not_bounded_retry_count() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(2);
                tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(2);
            },
            |_| {},
        );
        let mut state = PolicyControlState::default();
        let mut now = NOW;
        for attempt in 1_u16..=3 {
            let intent = intent(&catalog, u64::from(attempt));
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 100,
                        runtime_ms: 100,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                        failure_identity: None,
                        class: PolicyFailureClass::Recoverable,
                    },
                    &pressured(now + 100),
                )
                .expect("performance-associated failure");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert!(failure.performance_tax_exempt);
            assert_eq!(failure.consecutive_same_error, attempt);
            assert_eq!(failure.escalation_streak, 0);
            assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
            if attempt <= 2 {
                assert_eq!(
                    failure.disposition,
                    PolicyFailureDisposition::RetryScheduled
                );
            } else {
                assert_eq!(failure.disposition, PolicyFailureDisposition::Continue);
                assert_eq!(failure.retry_attempt, 0);
                assert!(failure.retry_at_unix_ms.is_none());
            }
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
            now = failure.retry_at_unix_ms.unwrap_or(now + 100);
        }
    }

    /// Workflow #361 M6: the startup reconciliation still records an interrupted run as a
    /// severe, paused failure, and the next admission of the pair is not held by it. Review
    /// P5: a second interruption in a row holds the pair as before.
    #[test]
    fn interrupted_settlement_does_not_hold_the_pair() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let mut interrupt = |suffix: u64, now: u64| {
            let interrupted = intent(&catalog, suffix);
            let admission = state
                .preview_admission(&catalog, &interrupted, now)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &interrupted,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 100,
                        runtime_ms: 0,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(SharedCode::PolicySettlementInterrupted),
                        failure_identity: None,
                        class: PolicyFailureClass::Severe,
                    },
                    &unavailable(now + 100),
                )
                .expect("interrupted settlement");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
            state
                .commit_execution(&catalog, &interrupted, &admission, &data)
                .expect("commit interrupted settlement");
            state
                .preview_admission(&catalog, &intent(&catalog, suffix + 1), now + 100)
                .map(|_| ())
                .map_err(|error| error.code())
        };
        assert_eq!(interrupt(1, NOW), Ok(()), "the pair is admitted again");
        assert_eq!(
            interrupt(2, NOW + 100),
            Err("policy_task_paused"),
            "a second interruption in a row holds the pair"
        );
    }

    /// W2-C (#419 R1; ISO-M rows A01, A05, E13-E15): a scheduled run that fails once with a
    /// code the contract's always-severe list named before Runtime 0.12.26 does not pause its
    /// task. Each code reaches the policy as ISO-2's run boundary records it: the kernel's code on
    /// top of the run's terminal and `failure_severe` false (a capture that met its session
    /// closing or its endpoint changed: `RuntimeHostError::instance_capture`; a frame commit or
    /// resolve its session refused: `RuntimeHostError::instance_operation`). The contract's class,
    /// as the live settlement, the startup recovery and the ledger take it
    /// (`scheduled_failure_class`), is `Recoverable`, so the policy schedules a retry: the pair
    /// is held by the back-off only, not paused, and is admitted again at the retry time.
    #[test]
    fn w2c_a_single_failure_with_a_former_always_severe_code_does_not_pause_its_task() {
        let catalog = catalog();
        for code in [
            ExecutionKernelCode::ExecutionSessionCloseInProgress,
            ExecutionKernelCode::ExecutionInstanceIdentityMismatch,
            ExecutionKernelCode::InputFrameCaptureMismatch,
            ExecutionKernelCode::InputFrameNotCommitted,
            ExecutionKernelCode::InputFrameSessionMissing,
        ] {
            let spelling = code.as_str();
            let outcome = Outcome::new(code.code());
            let class = actingcommand_contract::scheduled_failure_class(outcome.code(), false);
            assert_eq!(class, PolicyFailureClass::Recoverable, "{spelling}");
            let mut state = PolicyControlState::default();
            let first = intent(&catalog, 1);
            let admission = state
                .preview_admission(&catalog, &first, NOW)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &first,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: NOW + 100,
                        runtime_ms: 100,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome,
                        failure_identity: None,
                        class,
                    },
                    &unavailable(NOW + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("{spelling}: expected a failure")
            };
            assert_eq!(failure.outcome.code().as_str(), spelling);
            assert_eq!(
                failure.original_class,
                PolicyFailureClass::Recoverable,
                "{spelling}"
            );
            assert_eq!(
                failure.effective_class,
                PolicyFailureClass::Recoverable,
                "{spelling}"
            );
            assert_eq!(
                failure.disposition,
                PolicyFailureDisposition::RetryScheduled,
                "{spelling}"
            );
            let retry_at = failure.retry_at_unix_ms.expect("a retry time");
            state
                .commit_execution(&catalog, &first, &admission, &data)
                .expect("commit the failure");
            let next = intent(&catalog, 2);
            assert_eq!(
                state
                    .preview_admission(&catalog, &next, data.observed_at_unix_ms)
                    .expect_err("the retry back-off holds the pair")
                    .code(),
                "policy_retry_backoff_active",
                "{spelling}: the task is not paused"
            );
            assert!(
                state.preview_admission(&catalog, &next, retry_at).is_ok(),
                "{spelling}: the pair is admitted again at its retry time"
            );
        }
    }

    #[test]
    fn sensitive_failure_never_schedules_an_automatic_retry() {
        let catalog = catalog_with(
            |tasks| tasks["tasks"][0]["sensitive"] = serde_json::json!(true),
            |_| {},
        );
        let state = PolicyControlState::default();
        let intent = intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &intent, NOW)
            .expect("admission preview");
        let data = state
            .preview_execution(
                &catalog,
                &intent,
                &admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms: NOW + 100,
                    runtime_ms: 100,
                },
                &PolicyExecutionInput::Failed {
                    outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                    failure_identity: None,
                    class: PolicyFailureClass::Recoverable,
                },
                &unavailable(NOW + 100),
            )
            .expect("failure classification");
        let PolicyExecutionOutcome::Failed { failure } = data.outcome else {
            panic!("expected failure")
        };
        assert!(failure.sensitive);
        assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
        assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
        assert!(failure.retry_at_unix_ms.is_none());
    }

    #[test]
    fn failure_streak_requires_the_same_class_and_error() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let mut now = NOW;
        for (suffix, failure_code, class) in [
            (
                1,
                RuntimeErrorCode::CaptureFailed,
                PolicyFailureClass::Recoverable,
            ),
            (
                2,
                RuntimeErrorCode::BackendOperationFailed,
                PolicyFailureClass::Recoverable,
            ),
            (
                3,
                RuntimeErrorCode::CaptureFailed,
                PolicyFailureClass::Severe,
            ),
        ] {
            let intent = intent(&catalog, suffix);
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 100,
                        runtime_ms: 100,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(failure_code),
                        failure_identity: None,
                        class,
                    },
                    &unavailable(now + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert_eq!(failure.consecutive_same_error, 1);
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
            now = failure.retry_at_unix_ms.unwrap_or(now + 100);
        }
    }

    #[test]
    fn zero_configured_backoff_still_schedules_a_bounded_delay() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["on_failure"]["retry_backoff_ms"] = serde_json::json!(0);
                tasks["tasks"][0]["next_run_clamp_ms"] = serde_json::json!(5);
            },
            |_| {},
        );
        let state = PolicyControlState::default();
        let intent = intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &intent, NOW)
            .expect("admission preview");
        let data = state
            .preview_execution(
                &catalog,
                &intent,
                &admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms: NOW + 100,
                    runtime_ms: 100,
                },
                &PolicyExecutionInput::Failed {
                    outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                    failure_identity: None,
                    class: PolicyFailureClass::Recoverable,
                },
                &unavailable(NOW + 100),
            )
            .expect("failure classification");
        let PolicyExecutionOutcome::Failed { failure } = data.outcome else {
            panic!("expected failure")
        };
        assert_eq!(failure.retry_at_unix_ms, Some(NOW + 101));
    }

    #[test]
    fn exhausted_retry_limit_uses_the_declared_continue_or_pause_action() {
        for (action, expected) in [
            ("continue", PolicyFailureDisposition::Continue),
            ("pause", PolicyFailureDisposition::PausedTask),
        ] {
            let catalog = catalog_with(
                |tasks| {
                    tasks["tasks"][0]["on_failure"]["action"] = serde_json::json!(action);
                    tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(1);
                    tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(5);
                },
                |_| {},
            );
            let mut state = PolicyControlState::default();
            let mut now = NOW;
            for attempt in 1_u64..=2 {
                let intent = intent(&catalog, attempt);
                let admission = state
                    .preview_admission(&catalog, &intent, now)
                    .expect("admission preview");
                let data = state
                    .preview_execution(
                        &catalog,
                        &intent,
                        &admission,
                        PolicyExecutionTiming {
                            observed_at_unix_ms: now + 100,
                            runtime_ms: 100,
                        },
                        &PolicyExecutionInput::Failed {
                            outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                            failure_identity: None,
                            class: PolicyFailureClass::Recoverable,
                        },
                        &unavailable(now + 100),
                    )
                    .expect("failure classification");
                let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                    panic!("expected failure")
                };
                if attempt == 1 {
                    assert_eq!(
                        failure.disposition,
                        PolicyFailureDisposition::RetryScheduled
                    );
                } else {
                    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
                    assert_eq!(failure.disposition, expected);
                    assert!(failure.retry_at_unix_ms.is_none());
                }
                state
                    .commit_execution(&catalog, &intent, &admission, &data)
                    .expect("commit failure");
                now = failure.retry_at_unix_ms.unwrap_or(now + 100);
                if attempt == 2 {
                    let next = state.preview_admission(&catalog, &intent, now);
                    if expected == PolicyFailureDisposition::PausedTask {
                        assert_eq!(
                            next.expect_err("declared pause").code(),
                            "policy_task_paused"
                        );
                    } else {
                        let next = next.expect("declared continue admits the next run");
                        let success = state
                            .preview_execution(
                                &catalog,
                                &intent,
                                &next,
                                PolicyExecutionTiming {
                                    observed_at_unix_ms: now + 1,
                                    runtime_ms: 1,
                                },
                                &PolicyExecutionInput::Succeeded,
                                &unavailable(now + 1),
                            )
                            .expect("successful execution");
                        state
                            .commit_execution(&catalog, &intent, &next, &success)
                            .expect("success clears the failure disposition");
                        assert!(
                            !state.failure_streaks.contains_key(&(
                                intent.task_id.clone(),
                                intent.instance_id.clone()
                            ))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn runtime_owned_daily_budget_cannot_be_bypassed() {
        let catalog = catalog_with(
            |tasks| tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(2),
            |activity| activity["profiles"][0]["daily_budget"] = serde_json::json!(2),
        );
        let mut state = PolicyControlState::default();
        let mut replay = PolicyControlState::default();
        for index in 0..2 {
            let now = NOW + index * 600_000;
            let intent = intent(&catalog, index + 1);
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("budget admission");
            state
                .commit_admission(&catalog, &intent, &admission, index != 0)
                .expect("commit budget");
            replay
                .commit_admission(&catalog, &intent, &admission, index != 0)
                .unwrap();
            let execution = state.preview_execution(
                &catalog, &intent, &admission,
                PolicyExecutionTiming { observed_at_unix_ms: now + 100, runtime_ms: 100 },
                &PolicyExecutionInput::Failed {
                    outcome: Outcome::new(SharedCode::InputBackendOperationFailed),
                    failure_identity: Some(format!("input_backend_operation_failed~v2~k000000000000~m00000000aaaaaaaaaaaa~fu{index:012x}")),
                    class: PolicyFailureClass::Recoverable,
                },
                &unavailable(now + 100),
            ).unwrap();
            state
                .commit_execution(&catalog, &intent, &admission, &execution)
                .unwrap();
            replay
                .commit_execution(&catalog, &intent, &admission, &execution)
                .unwrap();
            // WF342 P specification: identity changes do not restore an exhausted round.
            for control in [&state, &replay] {
                assert_eq!(
                    control
                        .immediate_retry_pairs(&catalog, now + 100)
                        .unwrap()
                        .is_empty(),
                    index == 1
                );
                assert!(
                    control
                        .latest_failure(&intent.task_id, &intent.instance_id)
                        .is_some()
                );
            }
        }
        let error = state
            .preview_admission(&catalog, &intent(&catalog, 3), NOW + 1_200_000)
            .expect_err("third daily admission must fail");
        assert_eq!(error.code(), "policy_budget_exhausted");
        let rejection = error.policy_rejection();
        assert_eq!(
            rejection.budget,
            Some(PolicyBudgetDenial {
                dimension: PolicyBudgetDimension::TaskDaily,
                used: 2,
                requested: 1,
                limit: 2,
            })
        );
        let next = rejection.next_eligible_unix_ms.expect("next daily window");
        state
            .preview_admission(&catalog, &intent(&catalog, 3), next)
            .expect("same predicate permits the reported next time");
        for control in [&state, &replay] {
            assert!(
                control
                    .immediate_retry_pairs(&catalog, next)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                control
                    .validate_immediate_retry(&catalog, &intent(&catalog, 3), next)
                    .unwrap_err()
                    .code(),
                "policy_retry_round_ended"
            );
        }
        let next_intent = intent(&catalog, 3);
        let next_admission = state
            .preview_admission(&catalog, &next_intent, next)
            .unwrap();
        state
            .commit_admission(&catalog, &next_intent, &next_admission, false)
            .unwrap();
        assert_eq!(
            state.retry_rounds[&(next_intent.task_id.clone(), next_intent.instance_id.clone())]
                .admission
                .activity
                .admitted_at_unix_ms,
            next
        );

        // B11 first red: Workflow #269 issuecomment-5587376490. The shared
        // activity limits must be visible through the same read-only predicate.
        for (field, dimension, limit) in [
            ("daily_budget", PolicyBudgetDimension::ActivityDaily, 1),
            (
                "max_window_iterations",
                PolicyBudgetDimension::ActivityWindow,
                1,
            ),
            (
                "session_max_ms",
                PolicyBudgetDimension::ActivityRuntime,
                60_000,
            ),
        ] {
            let catalog = catalog_with(
                |_| {},
                |activity| {
                    activity["profiles"][0][field] = serde_json::json!(limit);
                },
            );
            let mut state = PolicyControlState::default();
            let first = intent(&catalog, 1);
            let admission = state.preview_admission(&catalog, &first, NOW).unwrap();
            state
                .commit_admission(&catalog, &first, &admission, false)
                .unwrap();
            let next_intent = intent(&catalog, 2);
            let error = state
                .preview_admission(&catalog, &next_intent, NOW + 600_000)
                .expect_err("shared activity budget excludes the candidate");
            let rejection = error.policy_rejection();
            assert_eq!(
                rejection.budget,
                Some(PolicyBudgetDenial {
                    dimension,
                    used: limit,
                    requested: limit,
                    limit,
                })
            );
            assert_eq!(
                state
                    .preview_admission(&catalog, &next_intent, NOW + 600_000)
                    .unwrap_err(),
                error,
                "availability preview must not consume counters or change the sample"
            );
            state
                .preview_admission(
                    &catalog,
                    &next_intent,
                    rejection.next_eligible_unix_ms.unwrap(),
                )
                .expect("reported next activity window passes the same admission predicate");
        }
    }

    #[test]
    fn runtime_owned_window_and_runtime_budgets_cannot_be_bypassed() {
        // WF342 P specification: ordinary/full-day/overnight occurrences and ordered replay.
        for (start, end, admitted_minute) in [(480, 1320, 1300), (0, 0, 1420), (1320, 120, 1430)] {
            let catalog = catalog_with(
                |_| {},
                |activity| {
                    activity["profiles"][0]["windows"][0]["start_minute_of_day"] =
                        serde_json::json!(start);
                    activity["profiles"][0]["windows"][0]["end_minute_of_day"] =
                        serde_json::json!(end);
                },
            );
            let mut state = PolicyControlState::default();
            let mut replay = PolicyControlState::default();
            let origin_at = NOW / 86_400_000 * 86_400_000 + admitted_minute * 60_000;
            let original_window =
                activity_window_at(&catalog.catalog().activity.profiles[0], origin_at)
                    .unwrap()
                    .unwrap();
            for index in 0..2 {
                let now = origin_at + index * 600_000;
                let dispatch = intent(&catalog, index + 1);
                let admission = state.preview_admission(&catalog, &dispatch, now).unwrap();
                assert_eq!(admission.activity.local_day, original_window.local_day);
                assert_eq!(admission.activity.window_id, original_window.window_id);
                state
                    .commit_admission(&catalog, &dispatch, &admission, index != 0)
                    .unwrap();
                replay
                    .commit_admission(&catalog, &dispatch, &admission, index != 0)
                    .unwrap();
                let execution = state.preview_execution(
                    &catalog, &dispatch, &admission,
                    PolicyExecutionTiming { observed_at_unix_ms: now + 100, runtime_ms: 100 },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(SharedCode::InputBackendOperationFailed),
                        failure_identity: Some(format!("input_backend_operation_failed~v2~k000000000000~m00000000aaaaaaaaaaaa~fu{index:012x}")),
                        class: PolicyFailureClass::Recoverable,
                    },
                    &unavailable(now + 100),
                ).unwrap();
                state
                    .commit_execution(&catalog, &dispatch, &admission, &execution)
                    .unwrap();
                replay
                    .commit_execution(&catalog, &dispatch, &admission, &execution)
                    .unwrap();
                let pair = (dispatch.task_id.clone(), dispatch.instance_id.clone());
                for control in [&state, &replay] {
                    assert_eq!(
                        control.retry_rounds[&pair]
                            .admission
                            .activity
                            .admitted_at_unix_ms,
                        origin_at
                    );
                    assert!(
                        control
                            .immediate_retry_pairs(&catalog, now + 100)
                            .unwrap()
                            .contains(&pair)
                    );
                    assert!(
                        control
                            .immediate_retry_pairs(&catalog, original_window.until_unix_ms)
                            .unwrap()
                            .is_empty()
                    );
                    assert_eq!(
                        control
                            .validate_immediate_retry(
                                &catalog,
                                &dispatch,
                                original_window.until_unix_ms
                            )
                            .unwrap_err()
                            .code(),
                        "policy_retry_round_ended"
                    );
                }
            }
        }
        let window_catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(4);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(1000000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
                activity["profiles"][0]["session_max_ms"] = serde_json::json!(1000000);
            },
        );
        let mut window_state = PolicyControlState::default();
        for index in 0..4 {
            let now = NOW + index * 600_000;
            let intent = intent(&window_catalog, index + 1);
            let admission = window_state
                .preview_admission(&window_catalog, &intent, now)
                .expect("window admission through the declared limit");
            assert_eq!(admission.budget.task_window_used, index as u32 + 1);
            window_state
                .commit_admission(&window_catalog, &intent, &admission, false)
                .expect("commit window budget");
        }
        let error = window_state
            .preview_admission(
                &window_catalog,
                &intent(&window_catalog, 5),
                NOW + 2_400_000,
            )
            .expect_err("fifth window iteration must fail when the limit is four");
        assert_eq!(error.code(), "policy_budget_exhausted");
        assert_eq!(
            error.policy_rejection().budget,
            Some(PolicyBudgetDenial {
                dimension: PolicyBudgetDimension::TaskWindow,
                used: 4,
                requested: 1,
                limit: 4,
            })
        );

        let runtime_catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(60000);
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(300000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
                activity["profiles"][0]["session_max_ms"] = serde_json::json!(300000);
            },
        );
        let mut runtime_state = PolicyControlState::default();
        for index in 0..4 {
            let now = NOW + index * 600_000;
            let intent = intent(&runtime_catalog, index + 1);
            let admission = runtime_state
                .preview_admission(&runtime_catalog, &intent, now)
                .expect("runtime admission through 300000ms cumulative usage");
            runtime_state
                .commit_admission(&runtime_catalog, &intent, &admission, false)
                .expect("commit runtime reservation");
            let execution = runtime_state
                .preview_execution(
                    &runtime_catalog,
                    &intent,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 75_000,
                        runtime_ms: 75_000,
                    },
                    &PolicyExecutionInput::Succeeded,
                    &unavailable(now + 75_000),
                )
                .expect("bounded runtime execution");
            runtime_state
                .commit_execution(&runtime_catalog, &intent, &admission, &execution)
                .expect("commit actual runtime");
        }
        let error = runtime_state
            .preview_admission(
                &runtime_catalog,
                &intent(&runtime_catalog, 5),
                NOW + 2_400_000,
            )
            .expect_err("runtime reservation crossing 300000ms must fail");
        assert_eq!(error.code(), "policy_budget_exhausted");
        assert_eq!(
            error.policy_rejection().budget,
            Some(PolicyBudgetDenial {
                dimension: PolicyBudgetDimension::TaskRuntime,
                used: 300_000,
                requested: 60_000,
                limit: 300_000,
            })
        );
    }

    #[test]
    fn actual_runtime_replaces_the_reservation_before_the_next_admission() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(30000);
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(100000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
                activity["profiles"][0]["session_max_ms"] = serde_json::json!(100000);
            },
        );
        for input in [
            PolicyExecutionInput::Succeeded,
            PolicyExecutionInput::Failed {
                outcome: Outcome::new(SharedCode::InputBackendOperationFailed),
                failure_identity: Some(
                    "input_backend_operation_failed~v2~k000000000000~m00000000aaaaaaaaaaaa~fu000000000001"
                        .to_owned(),
                ),
                class: PolicyFailureClass::Recoverable,
            },
        ] {
            let mut state = PolicyControlState::default();
            let first = intent(&catalog, 1);
            let admission = state
                .preview_admission(&catalog, &first, NOW)
                .expect("first admission");
            state
                .commit_admission(&catalog, &first, &admission, false)
                .expect("commit first admission");
            let execution = state
                .preview_execution(
                    &catalog,
                    &first,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: NOW + 80000,
                        runtime_ms: 80_000,
                    },
                    &input,
                    &unavailable(NOW + 80000),
                )
                .expect("first execution");
            state
                .commit_execution(&catalog, &first, &admission, &execution)
                .expect("commit actual runtime");

            let error = state
                .preview_admission(&catalog, &intent(&catalog, 2), NOW + 600000)
                .expect_err("actual runtime must constrain the next admission");
            assert_eq!(error.code(), "policy_budget_exhausted");
            assert!(
                state
                    .immediate_retry_pairs(&catalog, NOW + 600000)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                state.retry_rounds.is_empty(),
                "settled exhaustion ends the round"
            );
            assert_eq!(
                state
                    .latest_failure(&first.task_id, &first.instance_id)
                    .is_some(),
                matches!(input, PolicyExecutionInput::Failed { .. })
            );
        }
    }

    #[test]
    fn activity_sample_is_stable_and_recorded_before_cadence_blocks_reentry() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let first_intent = intent(&catalog, 1);
        let first = state
            .preview_admission(&catalog, &first_intent, NOW)
            .expect("first sample");
        let replay = state
            .preview_admission(&catalog, &first_intent, NOW)
            .expect("same-round sample");
        assert_eq!(first, replay);
        assert!(first.activity.seed > 0);
        state
            .commit_admission(&catalog, &first_intent, &first, false)
            .expect("commit sample");
        let error = state
            .preview_admission(&catalog, &intent(&catalog, 2), NOW)
            .expect_err("cadence must block immediate resampling");
        assert_eq!(error.code(), "policy_activity_interval_active");
        assert_eq!(
            error.policy_rejection().next_eligible_unix_ms,
            Some(first.activity.next_eligible_unix_ms)
        );
        state
            .preview_admission(
                &catalog,
                &intent(&catalog, 2),
                first.activity.next_eligible_unix_ms,
            )
            .expect("the sampled cadence boundary admits without resampling state");
    }

    /// Workflow #386 SR-2a: a run that a condition ended (an observation out of its sampling
    /// deadline, SCHED §4.1), as SR-2b's live settlement will record it.
    fn sampling_ended() -> PolicyExecutionInput {
        PolicyExecutionInput::Failed {
            outcome: Outcome::new(
                actingcommand_execution_kernel::ExecutionKernelCode::RecognitionSampleDeadlineExceeded
                    .code(),
            ),
            failure_identity: None,
            class: PolicyFailureClass::Recoverable,
        }
    }

    fn capture_failed() -> PolicyExecutionInput {
        PolicyExecutionInput::Failed {
            outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
            failure_identity: None,
            class: PolicyFailureClass::Recoverable,
        }
    }

    /// The end of the activity window `at_unix_ms` falls in: the bookkeeping window an
    /// occurrence expires with (SCHED §4.4).
    fn window_end(catalog: &CompiledCatalog, at_unix_ms: u64) -> u64 {
        activity_window_at(&catalog.catalog().activity.profiles[0], at_unix_ms)
            .expect("activity window")
            .expect("an open window")
            .until_unix_ms
    }

    /// Admits `intent` at `now` and settles its run after `runtime_ms`, committing both as the
    /// host does.
    fn settle(
        state: &mut PolicyControlState,
        catalog: &CompiledCatalog,
        intent: &DispatchIntent,
        now: u64,
        runtime_ms: u64,
        input: &PolicyExecutionInput,
        condition: Option<PolicyConditionSettlement>,
    ) -> (PolicyAdmissionRecord, PolicyExecutionEventData) {
        let admission = state
            .preview_admission(catalog, intent, now)
            .expect("admission");
        state
            .commit_admission(catalog, intent, &admission, false)
            .expect("commit the admission");
        let data = state
            .preview_settlement(
                catalog,
                intent,
                &admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms: now + runtime_ms,
                    runtime_ms,
                },
                input,
                &unavailable(now + runtime_ms),
                condition,
            )
            .expect("settlement");
        state
            .commit_execution(catalog, intent, &admission, &data)
            .expect("commit the settlement");
        (admission, data)
    }

    // Workflow #386 SR-2a, test row T6 (SCHED §4.2 items 1 and 2): a run that a condition ended
    // adds to no failure streak and escalates nothing. Its record keeps the pair's previous
    // counters and continues; the pair's latest real failure stays the one the next failure
    // counts on.
    #[test]
    fn sr2a_t6_a_condition_ending_adds_to_no_streak_and_keeps_the_latest_real_failure() {
        let catalog = catalog();
        let mut state = PolicyControlState::default();
        let first = intent(&catalog, 1);
        settle(
            &mut state,
            &catalog,
            &first,
            NOW,
            100,
            &capture_failed(),
            None,
        );

        let condition = PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: window_end(&catalog, NOW),
        };
        let (_, ended) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 2),
            NOW + 600_000,
            100,
            &sampling_ended(),
            Some(condition),
        );
        let PolicyExecutionOutcome::Failed { failure } = &ended.outcome else {
            panic!("expected a failure")
        };
        assert_eq!(failure.condition, Some(condition));
        assert_eq!(failure.consecutive_same_error, 1);
        assert_eq!(failure.escalation_streak, 1);
        assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
        assert_eq!(failure.disposition, PolicyFailureDisposition::Continue);
        assert_eq!(failure.retry_attempt, 0);
        assert_eq!(failure.retry_at_unix_ms, None);
        assert_eq!(
            state.latest_failure(&first.task_id, &first.instance_id),
            Some((None, first.decision_id.as_str()))
        );

        let (_, next) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 3),
            NOW + 1_200_000,
            100,
            &capture_failed(),
            None,
        );
        let PolicyExecutionOutcome::Failed { failure } = &next.outcome else {
            panic!("expected a failure")
        };
        assert_eq!(failure.consecutive_same_error, 2);
        assert_eq!(failure.escalation_streak, 2);
        assert_eq!(failure.condition, None);
    }

    // Workflow #386 SR-2a, test row T6 (SCHED §4.2 item 3): a run that a condition ended gives
    // back the task daily, task window, activity daily and activity window counts when its
    // record says `refunded`, so a task limited to one run a day is admitted again in the same
    // window. A record that does not say it gives nothing back, and the rerun is refused with
    // `policy_budget_exhausted`.
    #[test]
    fn sr2a_t6_a_refunded_condition_ending_gives_back_the_four_counts() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(1);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(1);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(1);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(1);
            },
        );
        for refunded in [true, false] {
            let mut state = PolicyControlState::default();
            let condition = PolicyConditionSettlement {
                refunded,
                count: 1,
                expires_at_unix_ms: window_end(&catalog, NOW),
            };
            settle(
                &mut state,
                &catalog,
                &intent(&catalog, 1),
                NOW,
                100,
                &sampling_ended(),
                Some(condition),
            );
            let rerun = state.preview_admission(&catalog, &intent(&catalog, 2), NOW + 600_000);
            if refunded {
                let budget = rerun.expect("the refunded counts admit the rerun").budget;
                assert_eq!(
                    (
                        budget.task_daily_used,
                        budget.task_window_used,
                        budget.activity_daily_used,
                        budget.activity_window_used,
                    ),
                    (1, 1, 1, 1)
                );
            } else {
                let error = rerun.expect_err("nothing was given back");
                assert_eq!(error.code(), "policy_budget_exhausted");
                assert_eq!(
                    error.policy_rejection().budget,
                    Some(PolicyBudgetDenial {
                        dimension: PolicyBudgetDimension::TaskDaily,
                        used: 1,
                        requested: 1,
                        limit: 1,
                    })
                );
            }
        }
    }

    // Workflow #386 SR-2a, test row T6b (SCHED §4.2 item 4): on a task shaped like W2 v14's
    // once-a-day tasks (`max_runtime_ms` 2,100,000), a run that a condition ended after
    // 1,900,000 ms adds no runtime, so its 600,000 ms rerun settles as a success: no
    // `policy_runtime_budget_exceeded`, no severe class, no paused task.
    #[test]
    fn sr2a_t6b_a_condition_ending_counts_no_runtime_against_its_rerun() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(1);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(1);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(2_100_000);
            },
            |_| {},
        );
        let mut state = PolicyControlState::default();
        let condition = PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: window_end(&catalog, NOW),
        };
        let (_, ended) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 1),
            NOW,
            1_900_000,
            &sampling_ended(),
            Some(condition),
        );
        let PolicyExecutionOutcome::Failed { failure } = &ended.outcome else {
            panic!("expected a failure")
        };
        assert_eq!(failure.condition, Some(condition));
        assert_eq!(failure.runtime_ms, 1_900_000);

        let (_, rerun) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 2),
            NOW + 2_400_000,
            600_000,
            &PolicyExecutionInput::Succeeded,
            None,
        );
        assert_eq!(
            rerun.outcome,
            PolicyExecutionOutcome::Succeeded {
                runtime_ms: 600_000
            }
        );
    }

    // Workflow #386 SR-2a, test row T6c (SCHED §4.2 item 0, N1): condition endings settled live
    // replay from their records alone. A fresh state commits the same admissions and execution
    // records without `policy_budget_receipt_mismatch` or `policy_execution_record_mismatch` and
    // rebuilds the same counts and the same streak.
    #[test]
    fn sr2a_t6c_condition_endings_replay_from_their_records() {
        let catalog = catalog();
        let expires_at_unix_ms = window_end(&catalog, NOW);
        let mut live = PolicyControlState::default();
        let mut records = Vec::new();
        for (suffix, input, condition) in [
            (1, capture_failed(), None),
            (
                2,
                sampling_ended(),
                Some(PolicyConditionSettlement {
                    refunded: true,
                    count: 1,
                    expires_at_unix_ms,
                }),
            ),
            (
                3,
                sampling_ended(),
                Some(PolicyConditionSettlement {
                    refunded: true,
                    count: 2,
                    expires_at_unix_ms,
                }),
            ),
            (4, capture_failed(), None),
        ] {
            let intent = intent(&catalog, suffix);
            let (admission, data) = settle(
                &mut live,
                &catalog,
                &intent,
                NOW + (suffix - 1) * 600_000,
                100,
                &input,
                condition,
            );
            records.push((intent, admission, data));
        }

        let mut replay = PolicyControlState::default();
        for (intent, admission, data) in &records {
            replay
                .commit_admission(&catalog, intent, admission, false)
                .expect("the admission replays");
            replay
                .commit_execution(&catalog, intent, admission, data)
                .expect("the execution replays from its record");
        }
        let next = intent(&catalog, 5);
        let at = NOW + 2_400_000;
        assert_eq!(
            replay
                .preview_admission(&catalog, &next, at)
                .expect("the replayed counts admit"),
            live.preview_admission(&catalog, &next, at)
                .expect("the live counts admit")
        );
        assert_eq!(
            replay.latest_failure(&next.task_id, &next.instance_id),
            live.latest_failure(&next.task_id, &next.instance_id)
        );
    }

    // Workflow #336 PKG-S3, M-PKG §4.4 S3-T2 (pause half): the same failure identity (format v2)
    // twice pauses the pair, and the pause is lifted against the main package and the layers that
    // identity names, with the content hashes recorded in it; nothing comes from the dispatch.
    #[test]
    fn s3_t2_the_same_identity_twice_pauses_on_the_packages_it_names() {
        use crate::failure_identity::{IdentityLayer, IdentityPackage};
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(1);
                tasks["tasks"][0]["on_failure"]["escalation_threshold"] = serde_json::json!(2);
            },
            |_| {},
        );
        let identity = "capture_failed~v2~k000000000000~m0123abcd456789abcdef\
                        ~p89abcdef0123456789ab~faaaaaaaaaaaa";
        let mut state = PolicyControlState::default();
        let mut now = NOW;
        for (suffix, expected) in [
            (1, PolicyFailureDisposition::RetryScheduled),
            (2, PolicyFailureDisposition::PausedTask),
        ] {
            let intent = intent(&catalog, suffix);
            let admission = state
                .preview_admission(&catalog, &intent, now)
                .expect("admission preview");
            let data = state
                .preview_execution(
                    &catalog,
                    &intent,
                    &admission,
                    PolicyExecutionTiming {
                        observed_at_unix_ms: now + 100,
                        runtime_ms: 100,
                    },
                    &PolicyExecutionInput::Failed {
                        outcome: Outcome::new(RuntimeErrorCode::CaptureFailed),
                        failure_identity: Some(identity.to_owned()),
                        class: PolicyFailureClass::Recoverable,
                    },
                    &unavailable(now + 100),
                )
                .expect("failure classification");
            let PolicyExecutionOutcome::Failed { failure } = &data.outcome else {
                panic!("expected failure")
            };
            assert_eq!(failure.disposition, expected, "failure {suffix}");
            assert_eq!(u64::from(failure.consecutive_same_error), suffix);
            state
                .commit_execution(&catalog, &intent, &admission, &data)
                .expect("commit failure");
            now = failure.retry_at_unix_ms.unwrap_or(now + 100);
        }
        let task = &catalog.catalog().tasks.tasks[0];
        let paused = state
            .failure_streaks
            .get(&(task.id.clone(), "fixture-instance-a".to_owned()))
            .and_then(|failure| failure.paused_on.clone())
            .expect("the pair is paused");
        assert_eq!(
            paused,
            PausedPackages {
                main: Some(IdentityPackage {
                    id_hash: "0123abcd".to_owned(),
                    hash: "456789abcdef".to_owned(),
                }),
                layers: vec![IdentityLayer::Declared(IdentityPackage {
                    id_hash: "89abcdef".to_owned(),
                    hash: "0123456789ab".to_owned(),
                })],
            }
        );
    }

    /// Workflow #422 LN-3: the value records of `state`, written as the host writes them and
    /// read back from their stored form.
    fn stored_value_records(state: &PolicyControlState) -> Vec<PolicyValueRecord> {
        use actingcommand_contract::{
            AuditInput, EventActor, EventPayload, EventSeverity, EventSource, OriginModule,
            PolicyPayload, PolicyPayloadDraft,
        };
        let events = crate::events::RuntimeEvents::new(
            b"ln3-fixture-salt",
            std::sync::Arc::new(crate::SystemRuntimeClock::new()),
        )
        .expect("runtime events");
        state
            .value_records(&BTreeMap::new(), &BTreeMap::new())
            .into_values()
            .map(|record| {
                let payload = match &record {
                    PolicyValueRecord::Control(value) => {
                        PolicyPayloadDraft::control_state_recorded(
                            value.as_ref().clone(),
                            AuditInput::new(),
                        )
                    }
                    PolicyValueRecord::Activity(value) => {
                        PolicyPayloadDraft::activity_state_recorded(
                            value.clone(),
                            AuditInput::new(),
                        )
                    }
                };
                let draft = events
                    .draft(
                        EventSeverity::Info,
                        EventSource::Scheduler,
                        OriginModule::Policy,
                        EventActor::Scheduler,
                        events.system_links().expect("system links"),
                        payload,
                    )
                    .expect("a value record draft");
                let stored = serde_json::to_value(
                    events
                        .sanitize(draft)
                        .expect("the value record sanitizes")
                        .payload(),
                )
                .expect("the stored form");
                let read: EventPayload =
                    serde_json::from_value(stored).expect("the stored form reads back");
                read.validate().expect("the stored record validates");
                match read {
                    EventPayload::Policy(PolicyPayload::ControlStateRecorded(value)) => {
                        PolicyValueRecord::Control(Box::new(value.state().clone()))
                    }
                    EventPayload::Policy(PolicyPayload::ActivityStateRecorded(value)) => {
                        PolicyValueRecord::Activity(value.state().clone())
                    }
                    other => panic!("a value record, not {:?}", other.event_type()),
                }
            })
            .collect()
    }

    // Workflow #422 LN-3, test row 2 (LEAN-L §5.1; register L7, L14; `ledger-observer.md`
    // §11.3): a restart from the value records alone keeps the runtime budgets and the activity
    // interval. Two runs of a pair use 90 s of its 100 s window runtime; the second admission's
    // sampled interval is still running. A state rebuilt from nothing but the records, as stored,
    // admits and refuses exactly as the state that wrote them: inside the interval, at its end
    // (the runtime budget refuses), and later in the window.
    #[test]
    fn ln3_b_a_restart_from_the_value_records_alone_keeps_runtime_budgets_and_activity_intervals() {
        let catalog = catalog_with(
            |tasks| {
                tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(30000);
                tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(10);
                tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = serde_json::json!(100000);
            },
            |activity| {
                activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
                activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
                activity["profiles"][0]["session_max_ms"] = serde_json::json!(1000000);
            },
        );
        let mut state = PolicyControlState::default();
        let (first, _) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 1),
            NOW,
            50_000,
            &PolicyExecutionInput::Succeeded,
            None,
        );
        let (second, _) = settle(
            &mut state,
            &catalog,
            &intent(&catalog, 2),
            first.activity.next_eligible_unix_ms,
            40_000,
            &PolicyExecutionInput::Succeeded,
            None,
        );

        let records = stored_value_records(&state);
        assert_eq!(records.len(), 2, "one pair and one activity profile");
        let mut restarted = PolicyControlState::default();
        for record in &records {
            restarted
                .apply_value_record(record)
                .expect("the record applies");
        }
        assert_eq!(
            restarted.value_records(&BTreeMap::new(), &BTreeMap::new()),
            state.value_records(&BTreeMap::new(), &BTreeMap::new()),
            "the records hold the complete state"
        );

        let third = intent(&catalog, 3);
        let eligible = second.activity.next_eligible_unix_ms;
        for at in [eligible - 1, eligible, NOW + 3_600_000] {
            match (
                state.preview_admission(&catalog, &third, at),
                restarted.preview_admission(&catalog, &third, at),
            ) {
                (Ok(live), Ok(reloaded)) => assert_eq!(reloaded, live, "at {at}"),
                (Err(live), Err(reloaded)) => {
                    assert_eq!(reloaded.code(), live.code(), "at {at}");
                    assert_eq!(
                        reloaded.policy_rejection(),
                        live.policy_rejection(),
                        "at {at}"
                    );
                }
                (live, reloaded) => panic!("at {at}: live {live:?}, restarted {reloaded:?}"),
            }
        }
        let interval = restarted
            .preview_admission(&catalog, &third, eligible - 1)
            .expect_err("the sampled interval still runs");
        assert_eq!(interval.code(), "policy_activity_interval_active");
        let budget = restarted
            .preview_admission(&catalog, &third, eligible)
            .expect_err("the window runtime is spent");
        assert_eq!(budget.code(), "policy_budget_exhausted");
        assert_eq!(
            budget.policy_rejection().budget,
            Some(PolicyBudgetDenial {
                dimension: PolicyBudgetDimension::TaskRuntime,
                used: 90_000,
                requested: 30_000,
                limit: 100_000,
            })
        );
    }
}
