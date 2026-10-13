// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #372 K2: a due clock slot that the policy does not dispatch leaves one trace, a
//! `goal_missed` planning signal whose outcome is its record's code (a refusal records at
//! `warning`, a deferral or an owed slot at `info`). A persistent gate refuses the slot
//! (`clock_slot_refused`); a transient gate defers it (`clock_slot_deferred`): an operator pause
//! at first sight, a queue wait or an unavailable instance only once the pair's activity window
//! has closed. A selected dispatch that serves an occurrence from before its window opened is
//! owed (`clock_slot_owed`). Nothing is kept beyond one evaluation: a record's identity is its
//! `signal_id`, checked against the planning-signal projection before it is written.

use super::{CatalogStore, matching_activity_profile};
use crate::RuntimeHostResult;
use crate::codes::HostCode;
use actingcommand_contract::outcome::{Code, Outcome, VocabularyToken};
use actingcommand_contract::{
    PolicyBudgetDimension, PolicyDispatchRejection, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind,
};
use actingcommand_policy::{
    ActivityWindowOccurrence, CandidateEligibility, CompiledCatalog, DispatchIntent,
    EligibilityState, EvaluationTime, InstanceSnapshot, PolicyCode, PolicyEvaluation,
    SchedulingDecisionState, TaskRuntimeSnapshot, activity_window_at, clock_slot_view,
};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

/// The closure's deferral of one (task, instance) pair in this evaluation: the gate's code and
/// class.
struct Deferral {
    code: Code,
    class: GateClass,
    next_wake_unix_ms: Option<u64>,
    budget_dimension: Option<PolicyBudgetDimension>,
}

/// The eligibility closure's deferrals, captured at its single return point. Memory only; it
/// lives as long as one evaluation.
#[derive(Default)]
pub(super) struct Deferrals {
    captured: RefCell<BTreeMap<(String, String), Deferral>>,
    budget_dimension: Cell<Option<PolicyBudgetDimension>>,
}

impl Deferrals {
    /// Wraps the eligibility closure: every deferral it returns under a host code is recorded
    /// for its pair (the closure defers only under host codes), and what it returns is passed on
    /// unchanged.
    pub(super) fn capture<E>(
        &self,
        mut eligibility: impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E>,
    ) -> impl FnMut(&DispatchIntent) -> Result<CandidateEligibility, E> {
        move |intent: &DispatchIntent| {
            self.budget_dimension.set(None);
            let result = eligibility(intent);
            if let Ok(CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms,
            }) = &result
                && let Some((code, class)) = closure_gate(&reason.outcome)
            {
                self.captured.borrow_mut().insert(
                    (intent.task_id.clone(), intent.instance_id.clone()),
                    Deferral {
                        code,
                        class,
                        next_wake_unix_ms: *next_wake_unix_ms,
                        budget_dimension: self.budget_dimension.take(),
                    },
                );
            }
            result
        }
    }

    /// The exhausted budget dimension of an availability denial, kept for the deferral the
    /// closure returns next.
    pub(super) fn note_budget(&self, rejection: &PolicyDispatchRejection) {
        self.budget_dimension
            .set(rejection.budget.as_ref().map(|budget| budget.dimension));
    }
}

/// The three record kinds; the word is the `signal_id` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotRecord {
    Refused,
    Deferred,
    Owed,
}

impl SlotRecord {
    const fn code(self) -> HostCode {
        match self {
            Self::Refused => HostCode::ClockSlotRefused,
            Self::Deferred => HostCode::ClockSlotDeferred,
            Self::Owed => HostCode::ClockSlotOwed,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Deferred => "deferred",
            Self::Owed => "owed",
        }
    }
}

/// The values of one record. Absent values are left out of its outcome.
#[derive(Debug, Clone, Default)]
pub(crate) struct SlotValues {
    pub(crate) gate_code: Option<Code>,
    pub(crate) budget_dimension: Option<PolicyBudgetDimension>,
    pub(crate) occurrence_unix_ms: u64,
    pub(crate) window_start_unix_ms: Option<u64>,
    pub(crate) window_end_unix_ms: Option<u64>,
    pub(crate) next_eligible_unix_ms: Option<u64>,
    pub(crate) next_occurrence_unix_ms: Option<u64>,
    pub(crate) first_run: bool,
}

/// `signal:clock_slot:<kind>:<hex>`, where `<hex>` is the lowercase SHA-256 of the UTF-8 bytes
/// `task \0 alias \0 occurrence`, the occurrence in decimal ASCII.
pub(crate) fn signal_id(
    kind: SlotRecord,
    task_id: &str,
    alias: &str,
    occurrence_unix_ms: u64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(task_id.as_bytes());
    digest.update([0]);
    digest.update(alias.as_bytes());
    digest.update([0]);
    digest.update(occurrence_unix_ms.to_string().as_bytes());
    format!("signal:clock_slot:{}:{:x}", kind.word(), digest.finalize())
}

/// The record's outcome: its code with the task, the alias and each present value (Workflow
/// #378 A2-P); an absent value is left out, and `first_run` is written only when true.
pub(crate) fn slot_outcome(
    kind: SlotRecord,
    task_id: &str,
    alias: &str,
    values: &SlotValues,
) -> Outcome {
    let mut outcome = Outcome::new(kind.code())
        .task_id(task_id)
        .alias(alias)
        .occurrence_unix_ms(values.occurrence_unix_ms);
    if let Some(gate_code) = values.gate_code {
        outcome = outcome.gate_code(gate_code);
    }
    if let Some(dimension) = values.budget_dimension {
        outcome = outcome.budget_dimension(BudgetDimensionToken(dimension));
    }
    if let Some(time) = values.window_start_unix_ms {
        outcome = outcome.window_start_unix_ms(time);
    }
    if let Some(time) = values.window_end_unix_ms {
        outcome = outcome.window_end_unix_ms(time);
    }
    if let Some(time) = values.next_eligible_unix_ms {
        outcome = outcome.next_eligible_unix_ms(time);
    }
    if let Some(time) = values.next_occurrence_unix_ms {
        outcome = outcome.next_occurrence_unix_ms(time);
    }
    if values.first_run {
        outcome = outcome.first_run(true);
    }
    outcome
}

/// The `goal_missed` planning signal of one record, observed at the evaluation time.
pub(crate) fn clock_slot_signal(
    kind: SlotRecord,
    task_id: &str,
    alias: &str,
    values: &SlotValues,
    observed_at_unix_ms: u64,
) -> PolicyPlanningSignalEventData {
    PolicyPlanningSignalEventData {
        signal_id: signal_id(kind, task_id, alias, values.occurrence_unix_ms),
        instance_id: alias.to_owned(),
        task_id: Some(task_id.to_owned()),
        kind: PolicyPlanningSignalKind::GoalMissed,
        outcome: slot_outcome(kind, task_id, alias, values),
        observed_at_unix_ms,
        detection_budget: None,
    }
}

/// A budget dimension as a token of the catalog vocabulary `budget_dimension`.
#[derive(Clone, Copy)]
struct BudgetDimensionToken(PolicyBudgetDimension);

impl VocabularyToken for BudgetDimensionToken {
    const VOCABULARY: &'static str = "budget_dimension";

    fn token(self) -> &'static str {
        dimension_token(self.0)
    }
}

/// `PolicyBudgetDimension`'s serde names.
const fn dimension_token(dimension: PolicyBudgetDimension) -> &'static str {
    match dimension {
        PolicyBudgetDimension::TaskDaily => "task_daily",
        PolicyBudgetDimension::TaskWindow => "task_window",
        PolicyBudgetDimension::TaskRuntime => "task_runtime",
        PolicyBudgetDimension::ActivityDaily => "activity_daily",
        PolicyBudgetDimension::ActivityWindow => "activity_window",
        PolicyBudgetDimension::ActivityRuntime => "activity_runtime",
    }
}

/// How a gate that stopped a due slot is traced (model §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateClass {
    /// Refused at first sight.
    Persistent,
    /// Deferred at first sight.
    OperatorPause,
    /// Deferred only at an evaluation where the pair's activity window is closed.
    QueueWait,
}

/// A closure deferral's gate: its host code and class. Any host code not named here is a
/// queue wait (the default rule); `None` for a reason that is not a host code.
fn closure_gate(reason: &Outcome) -> Option<(Code, GateClass)> {
    let code = HostCode::ALL
        .iter()
        .copied()
        .find(|code| reason.is(*code))?;
    let class = match code {
        HostCode::PolicyBudgetExhausted
        | HostCode::PolicyActivityWindowClosed
        | HostCode::PolicyTaskPaused => GateClass::Persistent,
        HostCode::DispatchPausedGlobal | HostCode::DispatchPausedInstance => {
            GateClass::OperatorPause
        }
        _ => GateClass::QueueWait,
    };
    Some((code.code(), class))
}

/// The placement blocks that are traced, with their class; other blocks are not.
const PLACEMENT_GATES: [(PolicyCode, GateClass); 4] = [
    (PolicyCode::InstanceOverrideDisabled, GateClass::Persistent),
    (PolicyCode::CapabilityMissing, GateClass::Persistent),
    (PolicyCode::ActivityProfileMissing, GateClass::Persistent),
    (PolicyCode::InstanceUnavailable, GateClass::QueueWait),
];

/// A placement block's gate: its code and class; `None` for a block that is not traced.
fn placement_gate(reason: &Outcome) -> Option<(Code, GateClass)> {
    PLACEMENT_GATES
        .iter()
        .find(|(code, _)| reason.is(*code))
        .map(|(code, class)| (code.code(), *class))
}

/// The gate that stopped a candidate, as received.
struct Gate {
    code: Code,
    class: GateClass,
    budget_dimension: Option<PolicyBudgetDimension>,
    next_eligible_unix_ms: Option<u64>,
}

struct Preview<'a> {
    store: &'a CatalogStore,
    catalog: &'a CompiledCatalog,
    tasks: &'a [TaskRuntimeSnapshot],
    instances: &'a [InstanceSnapshot],
    in_flight: &'a BTreeSet<(&'a str, &'a str)>,
    time: EvaluationTime,
    emitted: BTreeSet<String>,
    signals: Vec<PolicyPlanningSignalEventData>,
}

/// The clock-slot records of one evaluation, in the order they are to be appended: the
/// closure's deferrals, then the placement blocks, then the owed selections. Only pairs whose
/// trigger was true at this evaluation are read.
#[allow(clippy::too_many_arguments)]
pub(super) fn preview(
    store: &CatalogStore,
    catalog: &CompiledCatalog,
    tasks: &[TaskRuntimeSnapshot],
    instances: &[InstanceSnapshot],
    evaluation: &PolicyEvaluation,
    pending_dispatch_intents: &[DispatchIntent],
    in_flight: &BTreeSet<(&str, &str)>,
    deferrals: Deferrals,
    time: EvaluationTime,
) -> RuntimeHostResult<Vec<PolicyPlanningSignalEventData>> {
    let mut preview = Preview {
        store,
        catalog,
        tasks,
        instances,
        in_flight,
        time,
        emitted: BTreeSet::new(),
        signals: Vec::new(),
    };
    for ((task_id, alias), deferral) in deferrals.captured.into_inner() {
        let gate = Gate {
            code: deferral.code,
            class: deferral.class,
            budget_dimension: deferral.budget_dimension,
            next_eligible_unix_ms: deferral.next_wake_unix_ms,
        };
        preview.stopped(&task_id, &alias, gate)?;
    }
    for decision in &evaluation.decisions {
        if decision.state != SchedulingDecisionState::Blocked
            || decision.eligibility != EligibilityState::True
        {
            continue;
        }
        let (Some(alias), Some(reason)) =
            (decision.instance_id.as_deref(), decision.reasons.last())
        else {
            continue;
        };
        let Some((code, class)) = placement_gate(&reason.outcome) else {
            continue;
        };
        let gate = Gate {
            code,
            class,
            budget_dimension: None,
            next_eligible_unix_ms: None,
        };
        preview.stopped(&decision.task_id, alias, gate)?;
    }
    for intent in pending_dispatch_intents {
        preview.owed(&intent.task_id, &intent.instance_id)?;
    }
    Ok(preview.signals)
}

impl Preview<'_> {
    fn state(&self, task_id: &str, alias: &str) -> Option<&TaskRuntimeSnapshot> {
        self.tasks
            .iter()
            .find(|state| state.task_id == task_id && state.instance_id == alias)
    }

    /// Whether the instance has an activity profile, and the window open at the evaluation
    /// time. The profile is the evaluator's rule, the one an intent pins.
    fn window(&self, alias: &str) -> RuntimeHostResult<(bool, Option<ActivityWindowOccurrence>)> {
        let profile = self
            .instances
            .iter()
            .find(|instance| instance.instance_id == alias)
            .and_then(|instance| {
                matching_activity_profile(&self.catalog.catalog().activity.profiles, instance)
            });
        let Some(profile) = profile else {
            return Ok((false, None));
        };
        Ok((true, activity_window_at(profile, self.time.unix_ms)?))
    }

    /// The pair's due occurrence and next occurrence, when a counted clock has one due.
    fn slot(
        &self,
        task_id: &str,
        state: Option<&TaskRuntimeSnapshot>,
    ) -> RuntimeHostResult<Option<(u64, Option<u64>)>> {
        let Some(task) = self
            .catalog
            .catalog()
            .tasks
            .tasks
            .iter()
            .find(|task| task.id == task_id)
        else {
            return Ok(None);
        };
        Ok(clock_slot_view(task, state, self.time)?.and_then(|view| {
            view.due_occurrence
                .map(|occurrence| (occurrence, view.next_occurrence))
        }))
    }

    /// Whether a record with this id exists: written by this cycle, or in the projection.
    fn known(&self, signal_id: &str) -> RuntimeHostResult<bool> {
        Ok(self.emitted.contains(signal_id)
            || self.store.load_planning_signal(signal_id)?.is_some())
    }

    fn emit(&mut self, kind: SlotRecord, task_id: &str, alias: &str, values: &SlotValues) {
        let signal = clock_slot_signal(kind, task_id, alias, values, self.time.unix_ms);
        self.emitted.insert(signal.signal_id.clone());
        self.signals.push(signal);
    }

    /// A due slot stopped by a gate: refused or deferred, once per (pair, occurrence).
    fn stopped(&mut self, task_id: &str, alias: &str, gate: Gate) -> RuntimeHostResult<()> {
        if self.in_flight.contains(&(task_id, alias)) {
            return Ok(());
        }
        let (has_profile, window) = self.window(alias)?;
        // A queue wait is traced only once it outlasted the pair's activity window.
        if gate.class == GateClass::QueueWait && window.is_some() {
            return Ok(());
        }
        let state = self.state(task_id, alias);
        // A pair with no dispatch is traced only while its window is open, or with no profile;
        // its first slot is covered by the owed record of its first run.
        let dispatched = state.is_some_and(|state| state.last_dispatched_unix_ms.is_some());
        if !dispatched && has_profile && window.is_none() {
            return Ok(());
        }
        let Some((occurrence, next_occurrence)) = self.slot(task_id, state)? else {
            return Ok(());
        };
        let kind = match gate.class {
            GateClass::Persistent => SlotRecord::Refused,
            GateClass::OperatorPause | GateClass::QueueWait => SlotRecord::Deferred,
        };
        if self.known(&signal_id(kind, task_id, alias, occurrence))? {
            return Ok(());
        }
        // A refusal stops later deferrals of the same occurrence; a deferral stops only a later
        // refusal because the window closed, which is its consequence.
        let blocking = match kind {
            SlotRecord::Deferred => Some(SlotRecord::Refused),
            SlotRecord::Refused if gate.code == HostCode::PolicyActivityWindowClosed.code() => {
                Some(SlotRecord::Deferred)
            }
            SlotRecord::Refused | SlotRecord::Owed => None,
        };
        if let Some(blocking) = blocking
            && self.known(&signal_id(blocking, task_id, alias, occurrence))?
        {
            return Ok(());
        }
        let values = SlotValues {
            gate_code: Some(gate.code),
            budget_dimension: gate.budget_dimension,
            occurrence_unix_ms: occurrence,
            window_end_unix_ms: window.map(|window| window.until_unix_ms),
            next_eligible_unix_ms: gate.next_eligible_unix_ms,
            next_occurrence_unix_ms: next_occurrence,
            ..SlotValues::default()
        };
        self.emit(kind, task_id, alias, &values);
        Ok(())
    }

    /// A selection that serves an occurrence from before the window it is selected in.
    fn owed(&mut self, task_id: &str, alias: &str) -> RuntimeHostResult<()> {
        if self.in_flight.contains(&(task_id, alias)) {
            return Ok(());
        }
        let (_, Some(window)) = self.window(alias)? else {
            return Ok(());
        };
        let state = self.state(task_id, alias);
        let Some((occurrence, next_occurrence)) = self.slot(task_id, state)? else {
            return Ok(());
        };
        if occurrence >= window.from_unix_ms
            || self.known(&signal_id(SlotRecord::Owed, task_id, alias, occurrence))?
        {
            return Ok(());
        }
        let values = SlotValues {
            occurrence_unix_ms: occurrence,
            window_start_unix_ms: Some(window.from_unix_ms),
            next_occurrence_unix_ms: next_occurrence,
            first_run: state.is_none_or(|state| state.last_dispatched_unix_ms.is_none()),
            ..SlotValues::default()
        };
        self.emit(SlotRecord::Owed, task_id, alias, &values);
        Ok(())
    }
}
