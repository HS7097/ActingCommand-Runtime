// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S4 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.4, §3.7, §3.9, §6 S4):
//! the Runtime's side of the scheduler's fact refresh.
//!
//! - The start reads the marks of every recognition maintenance package it kept
//!   (`instances[].recognition_package`), once; each policy cycle hands the scheduler those
//!   readers with what the Runtime counts for them, the instances the closure's positions 1-2
//!   hold, and the `scheduling.fact_refresh.*` settings in effect.
//! - The closure tells a fact refresh's deferrals apart: a limit (the activity window or budget,
//!   a paused pair) or for now.
//! - A fact refresh dispatch runs its instance's package for the keys its reason chain requests
//!   (`fact_refresh_requested`), as any scheduled policy run, through the instance's one queue.

use crate::codes::HostCode;
use crate::policy_host::{InstanceOccupancy, PolicyRunContext};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{FactValue, PolicyBudgetDimension};
use actingcommand_execution_kernel::PreparedContainedTask;
use actingcommand_policy::{
    CandidateEligibility, DecisionReason, FACT_REFRESH_TASK_ID, InstanceHold, PolicyCode,
    RefreshLimit,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long the start waits to read one recognition maintenance package's marks.
const MARKS_DEADLINE: Duration = Duration::from_secs(60);

/// One instance's recognition maintenance package as the start kept it: where it is, and its
/// package id with the keys its reading marks write (D2 model §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FactRefreshBinding {
    pub(crate) path: PathBuf,
    /// `None` when the start could not read the package's marks: the binding serves no key, and
    /// every candidate that needs it says so (`fact_refresh_unbound`, `package_unreadable`).
    pub(crate) marks: Option<(String, BTreeSet<String>)>,
}

/// The marks of the kept bindings, read once at the start without preparing any package.
pub(crate) fn read_bindings(
    packages: &BTreeMap<String, PathBuf>,
) -> BTreeMap<String, FactRefreshBinding> {
    packages
        .iter()
        .map(|(alias, path)| {
            let marks = PreparedContainedTask::read_reading_marks(
                alias,
                path,
                Instant::now() + MARKS_DEADLINE,
            )
            .ok()
            .map(|marks| (marks.package_id, marks.fact_keys.into_iter().collect()));
            (
                alias.clone(),
                FactRefreshBinding {
                    path: path.clone(),
                    marks,
                },
            )
        })
        .collect()
}

/// The requested key set of a fact refresh run: the keys its reason chain requests
/// (`fact_refresh_requested`, D2 model §3.4); `None` for any other run, which reads every mark.
pub(crate) fn requested_keys(context: &PolicyRunContext) -> Option<BTreeSet<String>> {
    (context.catalog_task_id() == FACT_REFRESH_TASK_ID).then(|| {
        context
            .reason_chain()
            .reasons
            .iter()
            .filter(|reason| reason.outcome.is(PolicyCode::FactRefreshRequested))
            .filter_map(|reason| {
                reason
                    .outcome
                    .value("fact_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect()
    })
}

/// What a policy run reads from its own dispatch's `policy_dispatch_parameters`
/// ([`dispatch_parameters`]), with the dispatch's admission instant.
pub(crate) struct DispatchedParameters {
    /// Workflow #411 B15 R5: each `(parameter_name, parameter_value)` of `run_parameters`, in its
    /// order; `None` when the reason carries no `run_parameters` (a reason of Workflow #411 B8's
    /// values alone), so the run reads its package's declared defaults.
    pub(crate) values: Option<Vec<(String, String)>>,
    /// Workflow #411 B8: each schedule value the reason carries, `(key, value)`, typed.
    pub(crate) schedule: Vec<(String, FactValue)>,
    pub(crate) admitted_at_unix_ms: u64,
}

/// Workflow #411 B8: the keys of `policy_dispatch_parameters` a run reads as
/// `session.dispatch.schedule.<key>` (contracts/remedies.md section 4), with their fact types.
const DISPATCH_SCHEDULE_KEYS: [(&str, ScheduleValueKind); 5] = [
    ("next_occurrence_unix_ms", ScheduleValueKind::Timestamp),
    ("window_end_unix_ms", ScheduleValueKind::Timestamp),
    ("last_in_window", ScheduleValueKind::Boolean),
    ("last_daily_slot", ScheduleValueKind::Boolean),
    ("daily_remaining", ScheduleValueKind::Integer),
];

#[derive(Clone, Copy)]
enum ScheduleValueKind {
    Timestamp,
    Boolean,
    Integer,
}

/// Workflow #411 B15 R5 (contracts/remedies.md section 4): the run parameters a policy run reads
/// from its own dispatch reason chain, as a fact refresh reads its requested keys: each
/// `(parameter_name, parameter_value)` of the chain's `policy_dispatch_parameters`, in its order,
/// and (Workflow #411 B8) each schedule value it carries, with the dispatch's admission instant;
/// `None` when the chain carries none. An entry that is not a pair of strings, or a schedule value
/// of another type, gives nothing, so a package policy that reads it meets an unknown fact and
/// decides by its own `on_unknown`.
pub(crate) fn dispatch_parameters(context: &PolicyRunContext) -> Option<DispatchedParameters> {
    let reason = context
        .reason_chain()
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(PolicyCode::PolicyDispatchParameters))?;
    let values = reason
        .outcome
        .value("run_parameters")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let text = |key: &str| entry.get(key).and_then(serde_json::Value::as_str);
                    Some((
                        text("parameter_name")?.to_owned(),
                        text("parameter_value")?.to_owned(),
                    ))
                })
                .collect()
        });
    let schedule = DISPATCH_SCHEDULE_KEYS
        .iter()
        .filter_map(|(key, kind)| {
            let value = reason.outcome.value(key)?;
            let value = match kind {
                ScheduleValueKind::Timestamp => FactValue::TimestampMs(value.as_u64()?),
                ScheduleValueKind::Boolean => FactValue::Boolean(value.as_bool()?),
                ScheduleValueKind::Integer => FactValue::Integer(value.as_i64()?),
            };
            Some(((*key).to_owned(), value))
        })
        .collect();
    Some(DispatchedParameters {
        values,
        schedule,
        admitted_at_unix_ms: context.admission().activity.admitted_at_unix_ms,
    })
}

/// What the closure's positions 1-2 hold an instance with: an operator scheduling pause (no
/// wake: only `ResumeScheduling` lifts it), a holder or an eligible waiting claim (no wake: the
/// driver wakes when a key returns), or a takeover cooldown (a wake at its end).
pub(crate) struct Hold {
    gate: HostCode,
    reason: DecisionReason,
    next_wake_unix_ms: Option<u64>,
}

impl Hold {
    /// The closure's deferral of a candidate on the held instance; a fact refresh's names the
    /// gate, for now (D2 model §3.7 row 6).
    pub(crate) fn eligibility(&self, fact_refresh: bool) -> CandidateEligibility {
        if fact_refresh {
            CandidateEligibility::Refresh {
                reason: self.reason.clone(),
                next_wake_unix_ms: self.next_wake_unix_ms,
                gate: self.gate.code(),
                limit: None,
            }
        } else {
            CandidateEligibility::Deferred {
                reason: self.reason.clone(),
                next_wake_unix_ms: self.next_wake_unix_ms,
            }
        }
    }

    /// The hold as the scheduler's fact refresh reads it (D2 model §3.4 step 1).
    pub(crate) fn policy_hold(&self) -> InstanceHold {
        InstanceHold {
            reason: self.reason.clone(),
            next_wake_unix_ms: self.next_wake_unix_ms,
        }
    }
}

/// The closure's positions 1-2 for `instance_id` as the cycle starts, if they hold it.
pub(crate) fn instance_hold(
    instance_id: &str,
    pause: Option<HostCode>,
    occupancy: Option<InstanceOccupancy>,
) -> Option<Hold> {
    match (pause, occupancy) {
        (Some(code), _) => Some(Hold {
            gate: code,
            reason: DecisionReason {
                outcome: Outcome::new(code),
                detail: "Runtime scheduling paused by the operator".to_owned(),
            },
            next_wake_unix_ms: None,
        }),
        // Workflow #369 W-1: a held instance defers its candidates with no wake.
        (None, Some(InstanceOccupancy::Held { holder_kind, count })) => Some(Hold {
            gate: HostCode::DispatchInstanceHeld,
            reason: DecisionReason {
                outcome: Outcome::new(HostCode::DispatchInstanceHeld)
                    .instance_id(instance_id)
                    .holder_kind(crate::error::ClaimKindToken(holder_kind))
                    .count(count),
                detail: format!("holder_kind={} count={count}", holder_kind.as_str()),
            },
            next_wake_unix_ms: None,
        }),
        // A takeover cooldown defers with a wake at its end.
        (None, Some(InstanceOccupancy::Cooldown { until_unix_ms })) => Some(Hold {
            gate: HostCode::DispatchInstanceCooldown,
            reason: DecisionReason {
                outcome: Outcome::new(HostCode::DispatchInstanceCooldown)
                    .instance_id(instance_id)
                    .next_eligible_unix_ms(until_unix_ms),
                detail: format!("next_eligible_unix_ms={until_unix_ms}"),
            },
            next_wake_unix_ms: Some(until_unix_ms),
        }),
        (None, None) => None,
    }
}

/// A fact refresh's availability denial as the scheduler reads it (D2 model §3.7 rows 4 and 6):
/// a closed activity window or a spent activity budget, a spent task budget and a paused pair are
/// limits; anything else holds it back for now. `gate` is the denial's code.
pub(crate) fn refresh_denial(
    gate: Code,
    budget: Option<PolicyBudgetDimension>,
    reason: DecisionReason,
    next_wake_unix_ms: Option<u64>,
) -> CandidateEligibility {
    let limit = if gate == HostCode::PolicyActivityWindowClosed.code() {
        Some(RefreshLimit::ActivityWindow)
    } else if gate == HostCode::PolicyTaskPaused.code() {
        Some(RefreshLimit::FailurePause)
    } else if gate == HostCode::PolicyBudgetExhausted.code() {
        Some(match budget {
            Some(
                PolicyBudgetDimension::ActivityDaily
                | PolicyBudgetDimension::ActivityWindow
                | PolicyBudgetDimension::ActivityRuntime,
            ) => RefreshLimit::ActivityWindow,
            Some(
                PolicyBudgetDimension::TaskDaily
                | PolicyBudgetDimension::TaskWindow
                | PolicyBudgetDimension::TaskRuntime,
            )
            | None => RefreshLimit::DailyLimit,
        })
    } else {
        None
    };
    CandidateEligibility::Refresh {
        reason,
        next_wake_unix_ms,
        gate,
        limit,
    }
}
