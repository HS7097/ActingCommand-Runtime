// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #438 HELD-1 (register §1.322 rows HELD-R1 to HELD-R7, §1.327;
//! `contracts/scheduling/README.md`, "Held records"): a pair (task, instance) that should run and
//! has been held by one `why` code of the held set past the threshold in effect leaves one warning
//! record, `task_held`, and the end of that hold one info record, `task_held_ended`. Both are
//! `goal_missed` planning signals, each checked against the planning-signal projection before it
//! is written, so one hold is reported once and ended once, across restarts too.
//!
//! Nothing here decides anything: the records are found after the evaluation and its not-run
//! explanation, from that explanation's rows, and no decision reads them or the memory below. The
//! hold's start is the row's own `since_unix_ms` (the three waits for one fact's refresh of one
//! `fact_key` share one, `crate::schedule_explanation`). The memory holds the holds this run of the
//! Runtime reported and has not ended; a start begins with none, and its settlement ends the holds
//! an earlier run left open (`held_end` `restarted`, [`reported_hold`], [`restarted_end`]).

use crate::RuntimeHostResult;
use crate::codes::{ApplicationUpdateCode, HeldEnd, HeldState, HostCode, TaskHeldCode};
use crate::schedule_explanation::ScheduleExplanationTable;
use actingcommand_contract::outcome::{Code, CodeStr, Outcome};
use actingcommand_contract::{
    PolicyPlanningSignalEventData, PolicyPlanningSignalKind, ScheduleCandidateEligibility,
    ScheduleExplanationCandidate,
};
use actingcommand_policy::{
    FACT_REFRESH_TASK_ID, PolicyCode, PolicyEvaluation, SchedulingDecisionState,
    SchedulingRulePolicyCode,
};
use actingcommand_runtime_config::TaskHeldConfig;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The `signal_id` prefix of a held record: `signal:task_held:held:<hex>`.
pub(crate) const HELD_SIGNAL_PREFIX: &str = "signal:task_held:held:";
/// The `signal_id` prefix of the end of a hold: `signal:task_held:ended:<hex>`, the held record's
/// `<hex>`.
pub(crate) const ENDED_SIGNAL_PREFIX: &str = "signal:task_held:ended:";

/// The census's codes that are held whatever their values (register §1.322, settled 10): eleven
/// due (eligibility `true`), two undecidable (`unknown`) and the four fact refresh waits (either).
/// The held set is closed: a code a later slice adds is never held until the census names it.
pub(crate) const HELD_CODES: [Code; 17] = [
    // Held, due.
    PolicyCode::PolicyNoProgress.code(),
    PolicyCode::InstanceUnavailable.code(),
    PolicyCode::HeavySceneBudgetExhausted.code(),
    PolicyCode::HeavySceneBudgetDeferred.code(),
    PolicyCode::PolicyConcurrentRunsDeferred.code(),
    PolicyCode::InstanceAlreadySelected.code(),
    PolicyCode::PolicyChainHandoverWaiting.code(),
    HostCode::DispatchInstanceHeld.code(),
    ApplicationUpdateCode::Pending.code(),
    PolicyCode::SelectionInputUnknown.code(),
    SchedulingRulePolicyCode::DecisionReasonMissing.code(),
    // Held, undecidable.
    PolicyCode::TriggerUnknown.code(),
    PolicyCode::FeedbackStopUnknown.code(),
    // Held, either (by the row's eligibility).
    PolicyCode::FactRefreshPending.code(),
    PolicyCode::FactRefreshUnresolved.code(),
    PolicyCode::FactRefreshExhausted.code(),
    PolicyCode::FactRefreshUnbound.code(),
];

/// The value of `benefit_unknown` that is held: the benefit waits for a reading. The other gaps
/// (no effects, an entry or a valuation pending or missing) wait for the model to give a value and
/// are not held (register §1.322, settled 10).
const BENEFIT_GAP: &str = "benefit_gap";
const READING_UNKNOWN: &str = "reading_unknown";
/// The values of `pool_line_held` that are held: waiting for the order's head, or for a fact that
/// gives no value; `capped_not_dispatchable` is held when its own `gate_code` is.
const POOL_LINE_HOLD: &str = "pool_line_hold";
const CAPPED_NOT_HEAD: &str = "capped_not_head";
const CAPPED_CAP_UNKNOWN: &str = "capped_cap_unknown";
const CAPPED_NOT_DISPATCHABLE: &str = "capped_not_dispatchable";
const GATE_CODE: &str = "gate_code";
/// The values a held record carries that its end and the start's end pass read back.
const TASK_ID: &str = "task_id";
const ALIAS: &str = "alias";
const HELD_SINCE_UNIX_MS: &str = "held_since_unix_ms";

/// The held-record setting of an instance the evaluation's context does not name: off, so it
/// writes no new held record (the ends of its reported holds are still written).
const OFF: TaskHeldConfig = TaskHeldConfig {
    enabled: false,
    after_ms: u64::MAX,
};

/// One hold this run of the Runtime reported and has not ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenHold {
    since_unix_ms: u64,
    gate_code: Code,
}

/// The holds this run of the Runtime reported, by (instance alias, task), until their end is
/// written. Memory only; a start begins with none, and nothing decides by it.
#[derive(Debug, Default)]
pub(crate) struct HeldMemory {
    open: BTreeMap<(String, String), OpenHold>,
}

impl HeldMemory {
    /// The held records and ends of one evaluated cycle, in the order they are appended: the ends
    /// first, then the held records, after `table` was replaced from `evaluation`.
    ///
    /// - A reported hold ends `instance_gone` when the table has no entry for its instance,
    ///   `selected` when the evaluation selected the pair's decision, `row_gone` when the instance
    ///   has rows and none of the pair, and `why_changed` when the pair's row has another start
    ///   (another `why` code, or a fact refresh wait of another key). Its end is written only when
    ///   the projection holds its held record (a hold never reported leaves nothing) and not its
    ///   end, with `held_for_ms` from its start to now, whatever the instance's switch.
    /// - A candidate row is held when its task is no fact refresh read, the pair has no dispatch
    ///   in flight (`in_flight`, (task, alias)), its `why` is in the held set with eligibility
    ///   `true` or `unknown`, the instance's setting (`setting`, by alias; off when absent) is on
    ///   and `now_unix_ms` is at least its `since_unix_ms` plus `after_ms`. Its record is written
    ///   when the projection (`known`) does not hold it; a record the projection holds keeps the
    ///   hold open for its end.
    pub(crate) fn observe(
        &mut self,
        table: &ScheduleExplanationTable,
        evaluation: &PolicyEvaluation,
        in_flight: &BTreeSet<(&str, &str)>,
        setting: &dyn Fn(&str) -> Option<TaskHeldConfig>,
        known: &dyn Fn(&str) -> RuntimeHostResult<bool>,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<Vec<PolicyPlanningSignalEventData>> {
        let Some(rows) = table.candidate_rows() else {
            return Ok(Vec::new());
        };
        let selected = evaluation
            .decisions
            .iter()
            .filter(|decision| decision.state == SchedulingDecisionState::Selected)
            .filter_map(|decision| {
                Some((decision.task_id.as_str(), decision.instance_id.as_deref()?))
            })
            .collect::<BTreeSet<_>>();
        let mut signals = Vec::new();
        let ended = self
            .open
            .iter()
            .filter_map(|((alias, task_id), hold)| {
                let end = match rows.get(alias.as_str()) {
                    None => HeldEnd::InstanceGone,
                    Some(_) if selected.contains(&(task_id.as_str(), alias.as_str())) => {
                        HeldEnd::Selected
                    }
                    Some(candidates) => {
                        match candidates.iter().find(|row| row.task_id == *task_id) {
                            None => HeldEnd::RowGone,
                            Some(row) if row.since_unix_ms != hold.since_unix_ms => {
                                HeldEnd::WhyChanged
                            }
                            Some(_) => return None,
                        }
                    }
                };
                Some(((alias.clone(), task_id.clone()), end))
            })
            .collect::<Vec<_>>();
        for (key, end) in ended {
            let Some(hold) = self.open.remove(&key) else {
                continue;
            };
            let (alias, task_id) = key;
            if !known(&held_signal_id(&task_id, &alias, hold.since_unix_ms))? {
                continue;
            }
            let signal = ended_signal(
                &EndedHold {
                    task_id: &task_id,
                    alias: &alias,
                    gate_code: hold.gate_code,
                    since_unix_ms: hold.since_unix_ms,
                    end,
                    held_for_ms: Some(now_unix_ms.saturating_sub(hold.since_unix_ms)),
                },
                now_unix_ms,
            );
            if !known(&signal.signal_id)? {
                signals.push(signal);
            }
        }
        for (&alias, &candidates) in &rows {
            let setting = setting(alias).unwrap_or(OFF);
            for row in candidates {
                if row.task_id == FACT_REFRESH_TASK_ID
                    || in_flight.contains(&(row.task_id.as_str(), alias))
                {
                    continue;
                }
                let Some((gate_code, state)) = held(row) else {
                    continue;
                };
                let since_unix_ms = row.since_unix_ms;
                let due = setting.enabled
                    && now_unix_ms >= since_unix_ms.saturating_add(setting.after_ms);
                let key = (alias.to_owned(), row.task_id.clone());
                let open = self
                    .open
                    .get(&key)
                    .is_some_and(|hold| hold.since_unix_ms == since_unix_ms);
                if !open && !due {
                    continue;
                }
                let signal_id = held_signal_id(&row.task_id, alias, since_unix_ms);
                if known(&signal_id)? {
                    // Reported already: this run's record, or one the projection holds for the
                    // same start; it stays open for its end, unless its end is written too.
                    if !open && !known(&ended_signal_id(&row.task_id, alias, since_unix_ms))? {
                        self.open.insert(
                            key,
                            OpenHold {
                                since_unix_ms,
                                gate_code,
                            },
                        );
                    }
                    continue;
                }
                if !due {
                    continue;
                }
                signals.push(held_signal(
                    &HeldRow {
                        task_id: &row.task_id,
                        alias,
                        why: &row.why,
                        gate_code,
                        state,
                        since_unix_ms,
                        after_ms: setting.after_ms,
                    },
                    now_unix_ms,
                ));
                self.open.insert(
                    key,
                    OpenHold {
                        since_unix_ms,
                        gate_code,
                    },
                );
            }
        }
        Ok(signals)
    }
}

/// The held set's code of a candidate row and its `held_state`: `None` for a row whose `why` is
/// not in the held set, or whose eligibility is `false` (no held code comes with it).
pub(crate) fn held(row: &ScheduleExplanationCandidate) -> Option<(Code, HeldState)> {
    let gate_code = held_code(&row.why)?;
    let state = match row.eligibility {
        ScheduleCandidateEligibility::True => HeldState::Due,
        ScheduleCandidateEligibility::Unknown => HeldState::Undecidable,
        ScheduleCandidateEligibility::False => return None,
    };
    Some((gate_code, state))
}

/// The code of `why` when it is in the held set (register §1.322, the census): one of
/// [`HELD_CODES`]; `benefit_unknown` with `benefit_gap` `reading_unknown`; `pool_line_held` with
/// `pool_line_hold` `capped_not_head` or `capped_cap_unknown`, or `capped_not_dispatchable` whose
/// own `gate_code` is one of [`HELD_CODES`].
fn held_code(why: &Outcome) -> Option<Code> {
    if let Some(code) = HELD_CODES.iter().find(|code| why.is(**code)) {
        return Some(*code);
    }
    let text = |key: &str| why.value(key).and_then(Value::as_str);
    if why.is(PolicyCode::BenefitUnknown) {
        return (text(BENEFIT_GAP) == Some(READING_UNKNOWN))
            .then_some(PolicyCode::BenefitUnknown.code());
    }
    if why.is(PolicyCode::PoolLineHeld) {
        let held = match text(POOL_LINE_HOLD) {
            Some(CAPPED_NOT_HEAD | CAPPED_CAP_UNKNOWN) => true,
            Some(CAPPED_NOT_DISPATCHABLE) => text(GATE_CODE)
                .is_some_and(|gate| HELD_CODES.iter().any(|code| code.as_str() == gate)),
            _ => false,
        };
        return held.then_some(PolicyCode::PoolLineHeld.code());
    }
    None
}

/// The lowercase SHA-256 of the UTF-8 bytes `task \0 alias \0 since`, the start in decimal ASCII.
fn digest(task_id: &str, alias: &str, since_unix_ms: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(task_id.as_bytes());
    digest.update([0]);
    digest.update(alias.as_bytes());
    digest.update([0]);
    digest.update(since_unix_ms.to_string().as_bytes());
    format!("{:x}", digest.finalize())
}

/// `signal:task_held:held:<hex>` of the hold of (`task_id`, `alias`) that started at
/// `since_unix_ms`.
pub(crate) fn held_signal_id(task_id: &str, alias: &str, since_unix_ms: u64) -> String {
    format!(
        "{HELD_SIGNAL_PREFIX}{}",
        digest(task_id, alias, since_unix_ms)
    )
}

/// `signal:task_held:ended:<hex>`, the end of the same hold.
pub(crate) fn ended_signal_id(task_id: &str, alias: &str, since_unix_ms: u64) -> String {
    format!(
        "{ENDED_SIGNAL_PREFIX}{}",
        digest(task_id, alias, since_unix_ms)
    )
}

/// One held row as its record carries it.
pub(crate) struct HeldRow<'a> {
    pub(crate) task_id: &'a str,
    pub(crate) alias: &'a str,
    /// The row's `why` outcome, whole: the record's one `caused_by` link.
    pub(crate) why: &'a Outcome,
    pub(crate) gate_code: Code,
    pub(crate) state: HeldState,
    pub(crate) since_unix_ms: u64,
    pub(crate) after_ms: u64,
}

/// The held record of `row`, observed at the evaluation time `now_unix_ms`: `task_held` with
/// `task_id`, `alias`, `gate_code`, `held_state`, `held_since_unix_ms`, `held_for_ms` (now less
/// the start) and `held_after_ms`, and the row's `why` as its one `caused_by` link.
pub(crate) fn held_signal(row: &HeldRow<'_>, now_unix_ms: u64) -> PolicyPlanningSignalEventData {
    let outcome = Outcome::new(TaskHeldCode::Held)
        .task_id(row.task_id)
        .alias(row.alias)
        .gate_code(row.gate_code)
        .held_state(row.state)
        .held_since_unix_ms(row.since_unix_ms)
        .held_for_ms(now_unix_ms.saturating_sub(row.since_unix_ms))
        .held_after_ms(row.after_ms)
        .caused_by(row.why.clone());
    PolicyPlanningSignalEventData {
        signal_id: held_signal_id(row.task_id, row.alias, row.since_unix_ms),
        instance_id: row.alias.to_owned(),
        task_id: Some(row.task_id.to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome,
        observed_at_unix_ms: now_unix_ms,
        detection_budget: None,
    }
}

/// One ended hold as its end record carries it.
pub(crate) struct EndedHold<'a> {
    pub(crate) task_id: &'a str,
    pub(crate) alias: &'a str,
    pub(crate) gate_code: Code,
    pub(crate) since_unix_ms: u64,
    pub(crate) end: HeldEnd,
    /// From the start to the evaluation that found the hold ended; `None` for `restarted`.
    pub(crate) held_for_ms: Option<u64>,
}

/// The end of a hold, observed at `observed_at_unix_ms`: `task_held_ended` with `task_id`,
/// `alias`, `gate_code`, `held_since_unix_ms`, `held_end` and, when known, `held_for_ms`.
pub(crate) fn ended_signal(
    hold: &EndedHold<'_>,
    observed_at_unix_ms: u64,
) -> PolicyPlanningSignalEventData {
    let mut outcome = Outcome::new(TaskHeldCode::Ended)
        .task_id(hold.task_id)
        .alias(hold.alias)
        .gate_code(hold.gate_code)
        .held_since_unix_ms(hold.since_unix_ms)
        .held_end(hold.end);
    if let Some(held_for_ms) = hold.held_for_ms {
        outcome = outcome.held_for_ms(held_for_ms);
    }
    PolicyPlanningSignalEventData {
        signal_id: ended_signal_id(hold.task_id, hold.alias, hold.since_unix_ms),
        instance_id: hold.alias.to_owned(),
        task_id: Some(hold.task_id.to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome,
        observed_at_unix_ms,
        detection_budget: None,
    }
}

/// A held record read back from the ledger, for the start's end pass (register §1.322
/// HELD-R7): its task, alias, gate code and start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportedHold {
    pub(crate) task_id: String,
    pub(crate) alias: String,
    pub(crate) gate_code: Code,
    pub(crate) since_unix_ms: u64,
}

impl ReportedHold {
    /// The `signal_id` of this hold's end.
    pub(crate) fn ended_signal_id(&self) -> String {
        ended_signal_id(&self.task_id, &self.alias, self.since_unix_ms)
    }
}

/// What a planning signal is to the start's end pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeldRecordRead {
    /// No held record: its kind is not `goal_missed`, or its id has another prefix.
    Other,
    /// A held record HELD-1 writes, read back.
    Held(ReportedHold),
    /// A held record this build cannot read back: its outcome is not `task_held` with its task,
    /// alias, a `gate_code` the catalog this build embeds registers and its start, or its id is
    /// not the digest of its task, alias and start. The start's end pass leaves it as it is and
    /// names it on a developer line ([`unreadable_holds_line`]).
    Unreadable,
}

/// The registered code a held record's `gate_code` value names, by the catalog this build embeds,
/// so its end can carry it again. Whether this build's held set holds the code is not asked: the
/// set is closed per build, and a hold another build reported is ended all the same. `None` for a
/// value that is no code spelling of this build.
fn registered_gate_code(value: &Value) -> Option<Code> {
    serde_json::from_value::<CodeStr>(value.clone())
        .ok()?
        .registered()
}

/// One planning signal as the ledger or the cycle holds it.
pub(crate) struct SignalView<'a> {
    pub(crate) signal_id: &'a str,
    pub(crate) kind: PolicyPlanningSignalKind,
    pub(crate) task_id: Option<&'a str>,
    /// The record's `instance_id`, the instance alias.
    pub(crate) alias: &'a str,
    pub(crate) outcome: &'a Outcome,
}

/// What `signal` is to the start's end pass ([`HeldRecordRead`]).
pub(crate) fn reported_hold(signal: &SignalView<'_>) -> HeldRecordRead {
    if signal.kind != PolicyPlanningSignalKind::GoalMissed
        || !signal.signal_id.starts_with(HELD_SIGNAL_PREFIX)
    {
        return HeldRecordRead::Other;
    }
    let outcome = signal.outcome;
    let text = |key: &str| outcome.value(key).and_then(Value::as_str);
    let read = || {
        let task_id = signal.task_id?;
        let gate_code = outcome.value(GATE_CODE).and_then(registered_gate_code)?;
        let since_unix_ms = outcome.value(HELD_SINCE_UNIX_MS).and_then(Value::as_u64)?;
        let consistent = outcome.is(TaskHeldCode::Held)
            && text(TASK_ID) == Some(task_id)
            && text(ALIAS) == Some(signal.alias)
            && signal.signal_id == held_signal_id(task_id, signal.alias, since_unix_ms);
        consistent.then(|| ReportedHold {
            task_id: task_id.to_owned(),
            alias: signal.alias.to_owned(),
            gate_code,
            since_unix_ms,
        })
    };
    read().map_or(HeldRecordRead::Unreadable, HeldRecordRead::Held)
}

/// The start's end pass names at most this many unreadable held records on its developer line and
/// counts the rest. A bound of one log line, not a threshold.
const UNREADABLE_HOLDS_NAMED: usize = 8;

/// Workflow #438 HELD-1 (register §1.327): the developer line of the start's end pass for the
/// held records it could not read back ([`HeldRecordRead::Unreadable`]) and left as they are,
/// without an end: their ledger sequences, the first [`UNREADABLE_HOLDS_NAMED`] in ledger order
/// and the number of the rest; `None` when it read every held record. Fail Loud: a record the
/// start leaves open is never silent.
pub(crate) fn unreadable_holds_line(sequences: &[u64]) -> Option<String> {
    if sequences.is_empty() {
        return None;
    }
    let named = sequences
        .iter()
        .take(UNREADABLE_HOLDS_NAMED)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Some(format!(
        "actingd task_held unreadable sequences={} more={}",
        named.join(","),
        sequences.len().saturating_sub(named.len()),
    ))
}

/// The start's end of a hold an earlier run of the Runtime reported: `held_end` `restarted`,
/// without `held_for_ms` (the last evaluation of the run before is not known to the start).
pub(crate) fn restarted_end(
    hold: &ReportedHold,
    observed_at_unix_ms: u64,
) -> PolicyPlanningSignalEventData {
    ended_signal(
        &EndedHold {
            task_id: &hold.task_id,
            alias: &hold.alias,
            gate_code: hold.gate_code,
            since_unix_ms: hold.since_unix_ms,
            end: HeldEnd::Restarted,
            held_for_ms: None,
        },
        observed_at_unix_ms,
    )
}
