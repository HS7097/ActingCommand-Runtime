// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a2 (SCH-M frozen model v1.0 §3.2 D-3, D-4, D-6, §6 row SR-4a2;
//! `contracts/scheduling-blackout.md`, "Order of the deferrals", "Gradual quiet" and
//! "Day-boundary fit"): the policy side of blackouts, over the pure judgements of SR-4a1
//! (`actingcommand_policy::blackout_at`, `next_blackout_start`, `day_boundary_fit`).
//!
//! - Position 4: a blackout covers the instance as the cycle starts. Its candidates are deferred
//!   `blackout_active` with a wake at the blackout's end, and the instance is held for the fact
//!   refresh as positions 1-2 hold one.
//! - Position 7: the run's horizon plus `quiet_margin_ms` (or the hard quiet) reaches the next
//!   blackout start: `blackout_approaching`, a wake at that blackout's end. After it, on an
//!   instance whose `scheduling.day_boundary.quiet` is true, the run must end by its expected
//!   duration before the end of its activity window: `policy_day_boundary_approaching`, a wake
//!   at the next window start.
//! - The random delay's cut gains the quiet point, the latest start that still passes position
//!   7: the next blackout start minus the lead and, where the day-boundary fit applies, the
//!   window's end minus the expected duration E.
//! - Admission judges positions 4 and 7 again at its own time and refuses with the same codes.
//!
//! The three sources are the configuration's host list, each instance's own list and the active
//! catalog's `maintenance` events. With no entry and no such event nothing here defers, cuts or
//! refuses. Nothing here knows a game, a time of day or a rule: every interval is data.

use crate::codes::{HostLocation, SchedulingRuleHostCode};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{ContainedTaskRequest, RuntimeErrorCode};
use actingcommand_policy::{
    ActivityProfile, Blackout, BlackoutEntry, BlackoutSources, BlackoutWindow,
    CandidateEligibility, CompiledCatalog, DayBoundaryDeferral, DecisionReason, DispatchIntent,
    EvaluationTime, FACT_REFRESH_TASK_ID, InstanceHold, InstanceSnapshot, TimelineEventKind,
    activity_window_at, blackout_at, day_boundary_fit, next_blackout_start,
};
use actingcommand_runtime_config::scheduling::QuietHorizon;
use std::collections::BTreeMap;

/// The blackout keys in effect as a cycle or an admission starts, for the instances it reads
/// (`HostShared::blackout_context`).
#[derive(Debug, Clone)]
pub(crate) struct BlackoutContext {
    /// `scheduling.blackout.host`: every instance.
    pub(crate) host: Vec<BlackoutEntry>,
    /// Each instance's own list, by alias; an instance without entries is absent.
    pub(crate) instances: BTreeMap<String, Vec<BlackoutEntry>>,
    /// Each instance's `scheduling.day_boundary.quiet` in effect, by alias.
    pub(crate) day_boundary_quiet: BTreeMap<String, bool>,
    /// The section's `scheduling.day_boundary.quiet`, for an alias the map does not name.
    pub(crate) day_boundary_section: bool,
    pub(crate) quiet_margin_ms: u64,
    pub(crate) hard_quiet_ms: u64,
    pub(crate) quiet_horizon: QuietHorizon,
}

/// A configuration entry as the policy judgement reads it.
pub(crate) fn policy_blackout_entry(
    entry: &actingcommand_runtime_config::scheduling::BlackoutEntry,
) -> BlackoutEntry {
    use actingcommand_runtime_config::scheduling::BlackoutWindow as Configured;
    BlackoutEntry {
        id: entry.id.clone(),
        window: match &entry.window {
            Configured::Weekly {
                weekdays,
                utc_offset_minutes,
                start_minute_of_day,
                end_minute_of_day,
            } => BlackoutWindow::Weekly {
                weekdays: weekdays.clone(),
                utc_offset_minutes: *utc_offset_minutes,
                start_minute_of_day: *start_minute_of_day,
                end_minute_of_day: *end_minute_of_day,
            },
            Configured::Once {
                from_unix_ms,
                until_unix_ms,
            } => BlackoutWindow::Once {
                from_unix_ms: *from_unix_ms,
                until_unix_ms: *until_unix_ms,
            },
        },
    }
}

/// The response deadline of the request a policy dispatch runs under, as the evaluation knows it
/// before admission: a fact refresh read runs its instance's recognition maintenance package under
/// the request's default deadline (`RuntimeHost::fact_refresh_request`); every other scheduled run,
/// of a configured procedure or of a composed standard pack, under the bound each such request
/// carries (`ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`, set where actingd and the scheduler
/// workbench build them). Admission reads the request it is given instead.
pub(crate) fn evaluation_deadline_ms(intent: &DispatchIntent) -> u64 {
    if intent.task_id == FACT_REFRESH_TASK_ID {
        ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS
    } else {
        ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS
    }
}

/// Why a blackout keeps a candidate back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlackoutGate {
    /// Position 4: the blackout covers the instance now.
    Active(Blackout),
    /// Position 7: the run's horizon (`required_ms`) plus the margin reaches the next start.
    Approaching {
        blackout: Blackout,
        required_ms: u64,
    },
    /// Position 7, after the quiet: the run does not fit its activity window (SCH-M D-6).
    DayBoundary(DayBoundaryDeferral),
}

impl BlackoutGate {
    fn code(&self) -> Code {
        match self {
            Self::Active(_) => SchedulingRuleHostCode::BlackoutActive.code(),
            Self::Approaching { .. } => SchedulingRuleHostCode::BlackoutApproaching.code(),
            Self::DayBoundary(_) => SchedulingRuleHostCode::PolicyDayBoundaryApproaching.code(),
        }
    }

    /// When the candidate may run again: the blackout's end, or the next window start.
    pub(crate) fn wake_unix_ms(&self) -> u64 {
        match self {
            Self::Active(blackout) | Self::Approaching { blackout, .. } => blackout.end_unix_ms,
            Self::DayBoundary(deferral) => deferral.next_eligible_unix_ms,
        }
    }

    /// `outcome` (the gate's code, with or without an operation) with the gate's values;
    /// `instance_id` is the alias in an evaluation and the instance's opaque identifier at
    /// admission.
    fn with_values(&self, outcome: Outcome, instance_id: &str) -> Outcome {
        let outcome = outcome.instance_id(instance_id);
        match self {
            Self::Active(blackout) => outcome
                .blackout_id(blackout.id.as_str())
                .blackout_source(blackout.source)
                .next_eligible_unix_ms(blackout.end_unix_ms),
            Self::Approaching {
                blackout,
                required_ms,
            } => outcome
                .blackout_id(blackout.id.as_str())
                .blackout_source(blackout.source)
                .blackout_start_unix_ms(blackout.start_unix_ms)
                .required_ms(*required_ms),
            Self::DayBoundary(deferral) => outcome
                .window_end_unix_ms(deferral.window_end_unix_ms)
                .required_ms(deferral.required_ms)
                .next_eligible_unix_ms(deferral.next_eligible_unix_ms),
        }
    }

    fn reason(&self, instance_id: &str) -> DecisionReason {
        let outcome = match self {
            Self::Active(_) => Outcome::new(SchedulingRuleHostCode::BlackoutActive),
            Self::Approaching { .. } => Outcome::new(SchedulingRuleHostCode::BlackoutApproaching),
            Self::DayBoundary(_) => {
                Outcome::new(SchedulingRuleHostCode::PolicyDayBoundaryApproaching)
            }
        };
        let detail = match self {
            Self::Active(blackout) => format!(
                "Runtime blackout: blackout_id={}; blackout_source={:?}; next_eligible_unix_ms={}",
                blackout.id, blackout.source, blackout.end_unix_ms
            ),
            Self::Approaching {
                blackout,
                required_ms,
            } => format!(
                "Runtime blackout approaching: blackout_id={}; blackout_source={:?}; blackout_start_unix_ms={}; required_ms={required_ms}; next_eligible_unix_ms={}",
                blackout.id, blackout.source, blackout.start_unix_ms, blackout.end_unix_ms
            ),
            Self::DayBoundary(deferral) => format!(
                "Runtime day boundary: window_end_unix_ms={}; required_ms={}; next_eligible_unix_ms={}",
                deferral.window_end_unix_ms, deferral.required_ms, deferral.next_eligible_unix_ms
            ),
        };
        DecisionReason {
            outcome: self.with_values(outcome, instance_id),
            detail,
        }
    }

    /// The evaluation's deferral of a candidate on the instance `alias`; a fact refresh's names
    /// the gate, for now (D2 model §3.7 row 6).
    pub(crate) fn eligibility(&self, alias: &str, fact_refresh: bool) -> CandidateEligibility {
        let reason = self.reason(alias);
        let next_wake_unix_ms = Some(self.wake_unix_ms());
        if fact_refresh {
            CandidateEligibility::Refresh {
                reason,
                next_wake_unix_ms,
                gate: self.code(),
                limit: None,
            }
        } else {
            CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms,
            }
        }
    }

    /// The hold of an instance a blackout covers as the cycle starts, as the scheduler's fact
    /// refresh reads it: no candidate there raises a reading demand.
    pub(crate) fn hold(&self, alias: &str) -> InstanceHold {
        InstanceHold {
            reason: self.reason(alias),
            next_wake_unix_ms: Some(self.wake_unix_ms()),
        }
    }

    /// Admission's refusal (Info): the gate's code with its values, the instance named by its
    /// opaque identifier, and the wake as the rejection's `next_eligible_unix_ms`.
    pub(crate) fn admission_error(&self, instance_id: &str) -> RuntimeHostError {
        let mut error = RuntimeHostError::request(
            self.code(),
            HostLocation::AdmitPolicyDispatch,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_outcome_values(|outcome| self.with_values(outcome, instance_id));
        let mut rejection = error.policy_rejection();
        rejection.next_eligible_unix_ms = Some(self.wake_unix_ms());
        error.lifecycle.policy_rejection = Some(Box::new(rejection));
        error
    }
}

impl BlackoutContext {
    fn sources<'a>(&'a self, catalog: &'a CompiledCatalog) -> BlackoutSources<'a> {
        BlackoutSources {
            host: &self.host,
            instances: &self.instances,
            catalog: &catalog.catalog().timeline.events,
        }
    }

    /// Whether no source can cover any instance: no configured entry and no catalog
    /// `maintenance` event. Then no blackout judgement runs, so nothing changes.
    fn silent(&self, catalog: &CompiledCatalog) -> bool {
        self.host.is_empty()
            && self.instances.values().all(Vec::is_empty)
            && !catalog
                .catalog()
                .timeline
                .events
                .iter()
                .any(|event| event.event_kind == TimelineEventKind::Maintenance)
    }

    /// The horizon of the quiet for `intent` with the response deadline `deadline_ms`
    /// (`scheduling.blackout.quiet_horizon`): the deadline, or the intent's expected duration E.
    pub(crate) fn horizon_ms(&self, intent: &DispatchIntent, deadline_ms: u64) -> u64 {
        match self.quiet_horizon {
            QuietHorizon::Deadline => deadline_ms,
            QuietHorizon::Expected => intent.expected_duration_ms,
        }
    }

    /// How long before a blackout start a run of `horizon_ms` may last start (exclusive of the
    /// start): `horizon_ms + quiet_margin_ms`, and with a hard quiet at least `hard_quiet_ms + 1`
    /// (from `start - hard_quiet_ms` on nothing starts). Saturating: a longer lead only defers.
    fn lead_ms(&self, horizon_ms: u64) -> u64 {
        horizon_ms
            .saturating_add(self.quiet_margin_ms)
            .max(self.hard_quiet_ms.saturating_add(1))
    }

    /// Position 4 as the cycle starts: the blackout that covers each of `instances` at `time`,
    /// by alias.
    pub(crate) fn holds(
        &self,
        catalog: &CompiledCatalog,
        instances: &[InstanceSnapshot],
        time: EvaluationTime,
    ) -> RuntimeHostResult<BTreeMap<String, Blackout>> {
        let mut holds = BTreeMap::new();
        if self.silent(catalog) {
            return Ok(holds);
        }
        let sources = self.sources(catalog);
        for instance in instances {
            if let Some(blackout) = blackout_at(&sources, instance, time)? {
                holds.insert(instance.instance_id.clone(), blackout);
            }
        }
        Ok(holds)
    }

    /// Position 7 for `intent` on `instance` at `time`, its response deadline `deadline_ms`:
    /// the next blackout its lead reaches (`blackout_approaching`), else, on an instance with the
    /// day-boundary switch on, an activity window its expected duration does not fit
    /// (`policy_day_boundary_approaching`).
    pub(crate) fn quiet_gate(
        &self,
        catalog: &CompiledCatalog,
        instance: &InstanceSnapshot,
        intent: &DispatchIntent,
        time: EvaluationTime,
        deadline_ms: u64,
    ) -> RuntimeHostResult<Option<BlackoutGate>> {
        let required_ms = self.horizon_ms(intent, deadline_ms);
        if !self.silent(catalog) {
            let reach = time.unix_ms.saturating_add(self.lead_ms(required_ms));
            if let Some(blackout) =
                next_blackout_start(&self.sources(catalog), instance, time, reach)?
                    .filter(|next| next.start_unix_ms < reach)
            {
                return Ok(Some(BlackoutGate::Approaching {
                    blackout,
                    required_ms,
                }));
            }
        }
        match self.day_boundary_profile(catalog, instance, intent) {
            Some(profile) => {
                Ok(
                    day_boundary_fit(profile, time.unix_ms, intent.expected_duration_ms)?
                        .map(BlackoutGate::DayBoundary),
                )
            }
            None => Ok(None),
        }
    }

    /// The activity profile `intent` is admitted under when the day-boundary fit applies on
    /// `instance` (its `scheduling.day_boundary.quiet` in effect is true); `None` otherwise. A
    /// profile the catalog does not hold is the availability gate's to report (and admission's
    /// `policy_intent_catalog_mismatch`).
    fn day_boundary_profile<'a>(
        &self,
        catalog: &'a CompiledCatalog,
        instance: &InstanceSnapshot,
        intent: &DispatchIntent,
    ) -> Option<&'a ActivityProfile> {
        let quiet = self
            .day_boundary_quiet
            .get(&instance.instance_id)
            .copied()
            .unwrap_or(self.day_boundary_section);
        if !quiet {
            return None;
        }
        catalog
            .catalog()
            .activity
            .profiles
            .iter()
            .find(|profile| profile.id == intent.prerequisites.activity_profile_id)
    }

    /// Admission: positions 4 and 7 again for `intent` on `instance` at the admission's own
    /// `time`, with the response deadline of the request it is admitted with.
    pub(crate) fn admission_gate(
        &self,
        catalog: &CompiledCatalog,
        instance: &InstanceSnapshot,
        intent: &DispatchIntent,
        time: EvaluationTime,
        deadline_ms: u64,
    ) -> RuntimeHostResult<Option<BlackoutGate>> {
        if !self.silent(catalog)
            && let Some(blackout) = blackout_at(&self.sources(catalog), instance, time)?
        {
            return Ok(Some(BlackoutGate::Active(blackout)));
        }
        self.quiet_gate(catalog, instance, intent, time, deadline_ms)
    }

    /// The random delay's cut (`scheduling-blackout.md`, "Order of the deferrals", position 9)
    /// for a candidate that passed position 7 at `time`: the latest instant `intent` may start
    /// and still pass it. That is the earlier of the next blackout start minus the lead, for a
    /// blackout that begins within `reach_ms` (the longest delay) of that point now, and, where
    /// the day-boundary fit applies, the end of the open activity window minus the expected
    /// duration E; `None` when neither bounds the delay.
    pub(crate) fn quiet_point(
        &self,
        catalog: &CompiledCatalog,
        instance: &InstanceSnapshot,
        intent: &DispatchIntent,
        time: EvaluationTime,
        reach_ms: u64,
    ) -> RuntimeHostResult<Option<u64>> {
        let blackout = if self.silent(catalog) {
            None
        } else {
            let lead = self.lead_ms(self.horizon_ms(intent, evaluation_deadline_ms(intent)));
            let bound = time.unix_ms.saturating_add(lead).saturating_add(reach_ms);
            next_blackout_start(&self.sources(catalog), instance, time, bound)?
                .map(|next| next.start_unix_ms.saturating_sub(lead))
        };
        let day_boundary = match self.day_boundary_profile(catalog, instance, intent) {
            Some(profile) => activity_window_at(profile, time.unix_ms)?.map(|window| {
                window
                    .until_unix_ms
                    .saturating_sub(intent.expected_duration_ms)
            }),
            None => None,
        };
        Ok([blackout, day_boundary].into_iter().flatten().min())
    }
}
