// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B15, slice R2 (`contracts/remedies.md` sections 3.3, 4, 5, 6 and 7.2; model-b15
//! §5.3, §6, §7, §8, §9.2, §11): the evaluator's plan of the remedies packages declare.
//!
//! A policy run that succeeds on a mapped key whose mapping declares a remedy opens a round for
//! its (task, instance) pair. [`plan_remedies`] folds every round from the pairs' recent runs (the
//! journal the host keeps from the ledger), resolves which capabilities may serve the need (the
//! target record, else the configuration, else the package default) and plans, per pair, a
//! trigger override (a request of a provider, or the one retry of a served blocked pair), a
//! deferral (a hold, or a stop for a person), the run parameters of every pair whose package
//! declares parameters, and the stop, lift and expiry signals. [`crate::evaluate_with_remedies`]
//! applies the plan, and [`RemedyPlan::signals_after`] adds the stops the evaluation itself
//! decided: every requested provider behind a persistent gate.
//!
//! Needs, capabilities, parameters and options are opaque tokens, compared for equality only.
//! Nothing here pairs a need with a capability, orders capabilities or prefers one remedy: the
//! packages declare, the target record, the configuration or the package default allows, and the
//! existing ranking orders the requested providers.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::{
    Code, CodeStr, Outcome, Record, RemedyProblem, RemedySource,
};
use sha2::{Digest, Sha256};

use crate::codes::PolicyCode;
use crate::evaluator::{activity_scope_specificity, scope_matches_instance};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationTime, FactScalar, FactValue,
    InstanceSnapshot, ObservedFact, PolicyEvaluation, PolicyEvaluationError,
    PolicyEvaluationResult, SchedulingDecisionState, ScopeSelector, TaskSpec,
};

/// The instance fact that holds a target record (`contracts/instance-fact-store.md`, "Remedy
/// choice facts"), one per scope.
pub const REMEDY_CHOICES_FACT_KEY: &str = "session.remedy.choices";
/// A round's chain has at most this many layers; a provider of the last layer that ends blocked
/// stops the round (`depth_limit`).
pub const REMEDY_MAX_DEPTH: usize = 3;
/// The most runs of one pair a journal holds.
pub const REMEDY_JOURNAL_MAX_RUNS: usize = 64;
/// The most `policy_remedy_requested` reasons one dispatch carries.
pub const REMEDY_MAX_REQUESTED_REASONS: usize = 16;
/// The bounds of `scheduling.remedy.max_per_round`.
pub const REMEDY_MAX_PER_ROUND_BOUNDS: (u8, u8) = (1, 8);
/// The longest need, capability, parameter or option token.
const TOKEN_MAX_BYTES: usize = 64;

/// What one evaluation's remedy plan reads besides the evaluation inputs (model §9.2). Memory
/// only: the host rebuilds it from the packages, the ledger and the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyInputs {
    /// `scheduling.remedy.enabled`: off, the plan is empty.
    pub enabled: bool,
    /// `scheduling.remedy.max_per_round`: remedy requests per blocked layer and round.
    pub max_per_round: u8,
    pub table: RemedyTable,
    /// Per (task, instance), its recent runs.
    pub journal: Vec<RemedyJournal>,
    /// Per root pair, the latest stop signal not followed by a lift.
    pub stops: Vec<RemedyStop>,
    /// The `signal:remedy:` ids already recorded.
    pub signals_seen: BTreeSet<String>,
    /// Per instance alias, the configuration rows in effect (the instance's own list, else the
    /// section's).
    pub configuration: BTreeMap<String, Vec<RemedyChoiceRow>>,
    /// The host's codes of persistent gates an eligibility deferral can carry (the clock-slot
    /// table's `policy_task_paused` and `pack_admission_suspended`). The evaluator's own
    /// persistent placement codes (`instance_override_disabled`, `capability_missing`,
    /// `activity_profile_missing`) are always persistent.
    pub persistent_gates: Vec<Code>,
}

/// The remedy table of the active catalog (model §9.1), built by the host from the bound
/// packages' declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemedyTable {
    /// Per (task, outcome key), the remedy its mapping declares.
    pub remedies: Vec<RemedyTableRemedy>,
    /// Per task, the capabilities its package provides.
    pub providers: Vec<RemedyTableProvider>,
    /// The (blocked task, provider task) edges the host's cycle check refused.
    pub refused_edges: Vec<RemedyTableEdge>,
}

impl RemedyTable {
    pub fn is_empty(&self) -> bool {
        self.remedies.is_empty() && self.providers.is_empty()
    }
}

/// One mapping's `remedy` (`contracts/remedies.md` section 2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyTableRemedy {
    pub task_id: String,
    pub outcome_key: String,
    pub need: String,
    /// The declared capabilities, in declaration order.
    pub capabilities: Vec<String>,
    /// The package default; empty means none.
    pub default: Vec<String>,
    /// A finding the host's table holds for the mapping (`capability_unbound`, `cycle`,
    /// `parameters_mismatch`). The plan finds these per instance itself; it names this one when
    /// it finds no provider of its own.
    pub problem: Option<RemedyProblem>,
}

/// One task's `provides` (`contracts/remedies.md` section 2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyTableProvider {
    pub task_id: String,
    pub provides: Vec<RemedyTableCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyTableCapability {
    pub capability: String,
    pub irreversible: bool,
    pub parameters: Vec<RemedyTableParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyTableParameter {
    pub name: String,
    pub options: Vec<String>,
    pub default: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyTableEdge {
    pub blocked_task_id: String,
    pub provider_task_id: String,
}

/// One pair's runs, in ledger order, at most [`REMEDY_JOURNAL_MAX_RUNS`]: those since its
/// latest root blocked run, or its latest runs when it has none (model §7.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyJournal {
    pub task_id: String,
    pub instance_id: String,
    pub runs: Vec<RemedyRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyRun {
    pub decision_id: String,
    pub admitted_at_unix_ms: u64,
    /// The end of the activity window the run was admitted in.
    pub activity_window_end_unix_ms: u64,
    /// The `policy_remedy_requested` and `policy_remedy_retry` outcomes of its reason chain.
    pub remedy_reasons: Vec<Outcome>,
    pub settled: RemedySettlement,
    pub settled_at_unix_ms: Option<u64>,
}

/// How a run settled: not yet, successfully (with the key its terminal's disposition names,
/// whether or not the catalog references it), or as a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemedySettlement {
    InFlight,
    Succeeded { outcome_key: Option<String> },
    Failed,
}

/// A recorded stop of a round (its `signal:remedy:stopped:` planning signal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyStop {
    /// The round's root task.
    pub task_id: String,
    pub instance_id: String,
    /// The round: its root blocked run's decision.
    pub decision_id: String,
    pub remedy_inputs_digest: String,
    /// The recorded outcome, the stop code with its values.
    pub outcome: Outcome,
}

/// One row of a target record or of the configuration (`contracts/remedies.md` section 3.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemedyChoiceRow {
    /// `{row: "allow", need, capability}`.
    Allow { need: String, capability: String },
    /// `{row: "none", need}`.
    NoRemedy { need: String },
    /// `{row: "parameter", capability, parameter, value}`.
    Parameter {
        capability: String,
        parameter: String,
        value: String,
    },
}

impl RemedyChoiceRow {
    /// Reads one row of a `record_list`: `row` with exactly its fields, each a token string.
    pub fn from_fact_row(row: &BTreeMap<String, FactScalar>) -> Option<Self> {
        let text = |field: &str| match row.get(field) {
            Some(FactScalar::String(value)) if is_token(value) => Some(value.clone()),
            _ => None,
        };
        let Some(FactScalar::String(kind)) = row.get("row") else {
            return None;
        };
        let parsed = match kind.as_str() {
            "allow" if row.len() == 3 => Self::Allow {
                need: text("need")?,
                capability: text("capability")?,
            },
            "none" if row.len() == 2 => Self::NoRemedy {
                need: text("need")?,
            },
            "parameter" if row.len() == 4 => Self::Parameter {
                capability: text("capability")?,
                parameter: text("parameter")?,
                value: text("value")?,
            },
            _ => return None,
        };
        Some(parsed)
    }

    fn need(&self) -> Option<&str> {
        match self {
            Self::Allow { need, .. } | Self::NoRemedy { need } => Some(need),
            Self::Parameter { .. } => None,
        }
    }

    fn is_token_shaped(&self) -> bool {
        match self {
            Self::Allow { need, capability } => is_token(need) && is_token(capability),
            Self::NoRemedy { need } => is_token(need),
            Self::Parameter {
                capability,
                parameter,
                value,
            } => is_token(capability) && is_token(parameter) && is_token(value),
        }
    }

    fn hash_into(&self, hasher: &mut Sha256) {
        match self {
            Self::Allow { need, capability } => {
                for part in ["allow", need, capability] {
                    hash_part(hasher, part.as_bytes());
                }
            }
            Self::NoRemedy { need } => {
                for part in ["none", need] {
                    hash_part(hasher, part.as_bytes());
                }
            }
            Self::Parameter {
                capability,
                parameter,
                value,
            } => {
                for part in ["parameter", capability, parameter, value] {
                    hash_part(hasher, part.as_bytes());
                }
            }
        }
    }
}

/// The plan of one evaluation (model §9.2). Empty, it changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemedyPlan {
    /// Per (task, instance): treated as triggered and past its cooldown, and promoted.
    pub overrides: BTreeMap<(String, String), RemedyOverride>,
    /// Per (task, instance): deferred at position 8a with this reason.
    pub deferrals: BTreeMap<(String, String), RemedyDeferral>,
    /// Per (task, instance): the `policy_dispatch_parameters` reason a selected dispatch carries.
    pub run_parameters: BTreeMap<(String, String), DecisionReason>,
    /// The stop, lift and expiry signals the plan decided, none already recorded.
    pub signals: Vec<RemedySignal>,
    /// The layers that request providers this evaluation, for the persistent/transient split.
    requests: Vec<LayerRequest>,
    persistent_gates: Vec<Code>,
    signals_seen: BTreeSet<String>,
}

/// A trigger override of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemedyOverride {
    /// A provider requested for the blocked pairs it serves: one `policy_remedy_requested` per
    /// blocked pair, at most [`REMEDY_MAX_REQUESTED_REASONS`].
    Request {
        reasons: Vec<DecisionReason>,
        promoted: bool,
    },
    /// The one retry of a blocked pair whose remedy was served (`policy_remedy_retry`).
    Retry { reason: DecisionReason },
}

impl RemedyOverride {
    pub fn reasons(&self) -> &[DecisionReason] {
        match self {
            Self::Request { reasons, .. } => reasons,
            Self::Retry { reason } => std::slice::from_ref(reason),
        }
    }

    pub fn promoted(&self) -> bool {
        match self {
            Self::Request { promoted, .. } => *promoted,
            Self::Retry { .. } => true,
        }
    }
}

/// A deferral of the plan: a hold (`policy_remedy_waiting`, waking at the round's window end)
/// or a stop (`policy_remedy_unchosen`, `policy_remedy_unavailable`, `policy_remedy_exhausted`,
/// no wake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedyDeferral {
    pub reason: DecisionReason,
    pub next_wake_unix_ms: Option<u64>,
}

/// A `goal_missed` planning signal the host appends: `instance_id` the alias, `task_id` the
/// round's root task, the outcome the code with its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemedySignal {
    pub signal_id: String,
    pub task_id: String,
    pub instance_id: String,
    pub outcome: Outcome,
}

/// One layer that requests providers this evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LayerRequest {
    instance_id: String,
    root_task_id: String,
    blocked_task_id: String,
    providers: Vec<String>,
    /// The blocked pair's `policy_remedy_waiting`, before a gate is known.
    hold: Outcome,
    /// The root's `policy_remedy_unavailable` (`provider_unavailable`).
    stop: Outcome,
    signal_id: String,
}

/// How a requested provider left an evaluation.
pub(crate) struct ProviderView {
    pub(crate) selected: bool,
    pub(crate) last_code: Option<CodeStr>,
    pub(crate) next_wake_unix_ms: Option<u64>,
}

/// What became of a layer's requested providers.
enum LayerFate {
    Selected,
    Waiting {
        gate: Option<Code>,
        wake: Option<u64>,
    },
    Unavailable,
}

impl RemedyPlan {
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
            && self.deferrals.is_empty()
            && self.run_parameters.is_empty()
            && self.signals.is_empty()
            && self.requests.is_empty()
    }

    pub fn trigger_override(&self, task_id: &str, instance_id: &str) -> Option<&RemedyOverride> {
        pair_get(&self.overrides, task_id, instance_id)
    }

    pub fn deferral(&self, task_id: &str, instance_id: &str) -> Option<&RemedyDeferral> {
        pair_get(&self.deferrals, task_id, instance_id)
    }

    pub fn dispatch_parameters(&self, task_id: &str, instance_id: &str) -> Option<&DecisionReason> {
        pair_get(&self.run_parameters, task_id, instance_id)
    }

    /// The plan's signals and the stops the evaluation decided: a requesting layer none of whose
    /// providers was selected and every one of which met a persistent gate stops its round
    /// (`policy_remedy_unavailable`, `provider_unavailable`). None already recorded; ordered by
    /// id.
    pub fn signals_after(&self, evaluation: &PolicyEvaluation) -> Vec<RemedySignal> {
        let mut signals = self.signals.clone();
        for request in &self.requests {
            let fate = self.layer_fate(request, |task_id, instance_id| {
                evaluation
                    .decisions
                    .iter()
                    .find(|decision| {
                        decision.task_id == task_id
                            && decision.instance_id.as_deref() == Some(instance_id)
                    })
                    .map(|decision| ProviderView {
                        selected: decision.state == SchedulingDecisionState::Selected,
                        last_code: decision
                            .reasons
                            .last()
                            .map(|reason| reason.outcome.code().clone()),
                        next_wake_unix_ms: None,
                    })
            });
            if matches!(fate, LayerFate::Unavailable)
                && !self.signals_seen.contains(&request.signal_id)
            {
                signals.push(RemedySignal {
                    signal_id: request.signal_id.clone(),
                    task_id: request.root_task_id.clone(),
                    instance_id: request.instance_id.clone(),
                    outcome: request.stop.clone(),
                });
            }
        }
        signals.sort_by(|left, right| left.signal_id.cmp(&right.signal_id));
        signals.dedup_by(|left, right| left.signal_id == right.signal_id);
        signals
    }

    pub(crate) fn has_requests(&self) -> bool {
        !self.requests.is_empty()
    }

    /// The final reasons of the requesting layers' pairs once the allocation is known: the
    /// blocked pair's hold names the gate of its waiting provider (none when one was selected);
    /// when every provider met a persistent gate, the round's root stops instead.
    pub(crate) fn finalized_reasons(
        &self,
        view: impl Fn(&str, &str) -> Option<ProviderView>,
    ) -> BTreeMap<(String, String), RemedyDeferral> {
        let mut finals = BTreeMap::new();
        for request in &self.requests {
            match self.layer_fate(request, &view) {
                LayerFate::Selected => {}
                LayerFate::Waiting { gate, wake } => {
                    let mut hold = request.hold.clone();
                    if let Some(gate) = gate {
                        hold = hold.gate_code(gate);
                    }
                    if let Some(wake) = wake {
                        hold = hold.next_eligible_unix_ms(wake);
                    }
                    let wake = self
                        .deferral(&request.blocked_task_id, &request.instance_id)
                        .and_then(|deferral| deferral.next_wake_unix_ms);
                    finals.insert(
                        (request.blocked_task_id.clone(), request.instance_id.clone()),
                        RemedyDeferral {
                            reason: outcome_reason(hold),
                            next_wake_unix_ms: wake,
                        },
                    );
                }
                LayerFate::Unavailable => {
                    finals.insert(
                        (request.root_task_id.clone(), request.instance_id.clone()),
                        RemedyDeferral {
                            reason: outcome_reason(request.stop.clone()),
                            next_wake_unix_ms: None,
                        },
                    );
                }
            }
        }
        finals
    }

    fn layer_fate<V: Fn(&str, &str) -> Option<ProviderView>>(
        &self,
        request: &LayerRequest,
        view: V,
    ) -> LayerFate {
        // The transient gate with the earliest wake, the first provider's on a tie.
        let mut waiting: Option<(Option<Code>, Option<u64>)> = None;
        for provider in &request.providers {
            let (gate, wake) = match view(provider.as_str(), request.instance_id.as_str()) {
                Some(seen) if seen.selected => return LayerFate::Selected,
                Some(seen) => {
                    let gate = seen.last_code.as_ref().and_then(CodeStr::registered);
                    if gate.is_some_and(|gate| self.is_persistent(gate)) {
                        continue;
                    }
                    (gate, seen.next_wake_unix_ms)
                }
                None => (None, None),
            };
            let earlier = match &waiting {
                None => true,
                Some((_, current)) => match (wake, current) {
                    (Some(wake), Some(current)) => wake < *current,
                    (Some(_), None) => true,
                    _ => false,
                },
            };
            if earlier {
                waiting = Some((gate, wake));
            }
        }
        match waiting {
            Some((gate, wake)) => LayerFate::Waiting { gate, wake },
            None => LayerFate::Unavailable,
        }
    }

    fn is_persistent(&self, gate: Code) -> bool {
        [
            PolicyCode::InstanceOverrideDisabled.code(),
            PolicyCode::CapabilityMissing.code(),
            PolicyCode::ActivityProfileMissing.code(),
        ]
        .contains(&gate)
            || self.persistent_gates.contains(&gate)
    }

    fn push_signal(
        &mut self,
        signal_id: String,
        task_id: &str,
        instance_id: &str,
        outcome: Outcome,
    ) {
        if !self.signals_seen.contains(&signal_id) {
            self.signals.push(RemedySignal {
                signal_id,
                task_id: task_id.to_owned(),
                instance_id: instance_id.to_owned(),
                outcome,
            });
        }
    }
}

/// Plans the remedies of one evaluation: pure and deterministic. With remedies off or an empty
/// table the plan is empty.
pub fn plan_remedies(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    time: EvaluationTime,
    inputs: &RemedyInputs,
) -> PolicyEvaluationResult<RemedyPlan> {
    if !inputs.enabled || inputs.table.is_empty() {
        return Ok(RemedyPlan::default());
    }
    validate_remedy_inputs(inputs)?;
    let table = TableIndex::new(&inputs.table);
    let mut plan = RemedyPlan {
        persistent_gates: inputs.persistent_gates.clone(),
        signals_seen: inputs.signals_seen.clone(),
        ..RemedyPlan::default()
    };
    for instance in &facts.instances {
        let configuration = inputs
            .configuration
            .get(&instance.instance_id)
            .map_or(&[][..], Vec::as_slice);
        let choices = InstanceChoices::new(facts, instance, configuration, time);
        let context = InstanceContext {
            catalog,
            instance,
            choices: &choices,
            table: &table,
            inputs,
            time,
        };
        context.plan_rounds(&mut plan);
        context.plan_run_parameters(&mut plan);
    }
    plan.signals
        .sort_by(|left, right| left.signal_id.cmp(&right.signal_id));
    plan.signals
        .dedup_by(|left, right| left.signal_id == right.signal_id);
    Ok(plan)
}

fn validate_remedy_inputs(inputs: &RemedyInputs) -> PolicyEvaluationResult<()> {
    let (low, high) = REMEDY_MAX_PER_ROUND_BOUNDS;
    if !(low..=high).contains(&inputs.max_per_round) {
        return Err(PolicyEvaluationError::invalid(format!(
            "remedy max_per_round {} is outside {low}..={high}",
            inputs.max_per_round
        )));
    }
    let mut pairs = BTreeSet::new();
    for journal in &inputs.journal {
        if journal.runs.len() > REMEDY_JOURNAL_MAX_RUNS {
            return Err(PolicyEvaluationError::invalid(format!(
                "remedy journal of task '{}' on '{}' holds {} runs, more than {REMEDY_JOURNAL_MAX_RUNS}",
                journal.task_id,
                journal.instance_id,
                journal.runs.len()
            )));
        }
        if !pairs.insert((journal.task_id.as_str(), journal.instance_id.as_str())) {
            return Err(PolicyEvaluationError::invalid(format!(
                "remedy journal of task '{}' on '{}' is given twice",
                journal.task_id, journal.instance_id
            )));
        }
    }
    let table = &inputs.table;
    if let Some(remedy) = table
        .remedies
        .iter()
        .find(|remedy| remedy.capabilities.is_empty())
    {
        return Err(PolicyEvaluationError::invalid(format!(
            "remedy of task '{}' on '{}' declares no capability",
            remedy.task_id, remedy.outcome_key
        )));
    }
    let tokens = table
        .remedies
        .iter()
        .flat_map(|remedy| {
            [&remedy.outcome_key, &remedy.need]
                .into_iter()
                .chain(&remedy.capabilities)
                .chain(&remedy.default)
        })
        .chain(table.providers.iter().flat_map(|provider| {
            provider.provides.iter().flat_map(|capability| {
                std::iter::once(&capability.capability).chain(
                    capability.parameters.iter().flat_map(|parameter| {
                        std::iter::once(&parameter.name)
                            .chain(&parameter.options)
                            .chain(std::iter::once(&parameter.default))
                    }),
                )
            })
        }));
    for token in tokens {
        if !is_token(token) {
            return Err(PolicyEvaluationError::invalid(format!(
                "remedy table names '{token}', which is not a token"
            )));
        }
    }
    Ok(())
}

/// `^[a-z][a-z0-9_]{0,63}$`.
fn is_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= TOKEN_MAX_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn pair_get<'a, V>(
    map: &'a BTreeMap<(String, String), V>,
    task_id: &str,
    instance_id: &str,
) -> Option<&'a V> {
    if map.is_empty() {
        return None;
    }
    map.get(&(task_id.to_owned(), instance_id.to_owned()))
}

/// The table, indexed.
struct TableIndex<'a> {
    remedies: BTreeMap<(&'a str, &'a str), &'a RemedyTableRemedy>,
    provides: BTreeMap<&'a str, &'a [RemedyTableCapability]>,
    refused: BTreeSet<(&'a str, &'a str)>,
}

impl<'a> TableIndex<'a> {
    fn new(table: &'a RemedyTable) -> Self {
        Self {
            remedies: table
                .remedies
                .iter()
                .map(|remedy| {
                    (
                        (remedy.task_id.as_str(), remedy.outcome_key.as_str()),
                        remedy,
                    )
                })
                .collect(),
            provides: table
                .providers
                .iter()
                .map(|provider| (provider.task_id.as_str(), provider.provides.as_slice()))
                .collect(),
            refused: table
                .refused_edges
                .iter()
                .map(|edge| {
                    (
                        edge.blocked_task_id.as_str(),
                        edge.provider_task_id.as_str(),
                    )
                })
                .collect(),
        }
    }

    fn remedy(&self, task_id: &str, outcome_key: &str) -> Option<&'a RemedyTableRemedy> {
        self.remedies.get(&(task_id, outcome_key)).copied()
    }

    fn provided(&self, task_id: &str, capability: &str) -> Option<&'a RemedyTableCapability> {
        self.provides
            .get(task_id)
            .copied()?
            .iter()
            .find(|provided| provided.capability == capability)
    }
}

/// The rows one source holds for an instance.
enum ChoiceRows {
    Absent,
    /// A record or a list that cannot be read: it decides every need as `choice_invalid`.
    Unreadable,
    Rows(Vec<RemedyChoiceRow>),
}

impl ChoiceRows {
    /// The values a source gives a capability's parameter: none, one, or a conflict.
    fn parameter_values(&self, capability: &str, parameter: &str) -> Vec<&str> {
        let ChoiceRows::Rows(rows) = self else {
            return Vec::new();
        };
        rows.iter()
            .filter_map(|row| match row {
                RemedyChoiceRow::Parameter {
                    capability: row_capability,
                    parameter: row_parameter,
                    value,
                } if row_capability == capability && row_parameter == parameter => {
                    Some(value.as_str())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

/// The choices that apply to one instance (model §5.1, §5.2).
struct InstanceChoices<'a> {
    /// The most specific target record, as a whole.
    target: ChoiceRows,
    /// Every target record that applies (instance, server, game), most specific first: the
    /// versions the inputs digest covers.
    records: Vec<&'a ObservedFact>,
    configuration: ChoiceRows,
    configuration_rows: &'a [RemedyChoiceRow],
}

impl<'a> InstanceChoices<'a> {
    fn new(
        facts: &'a EvaluationFacts,
        instance: &InstanceSnapshot,
        configuration: &'a [RemedyChoiceRow],
        time: EvaluationTime,
    ) -> Self {
        let mut records = facts
            .facts
            .iter()
            .filter(|fact| {
                fact.fact_key == REMEDY_CHOICES_FACT_KEY
                    && scope_matches_instance(&fact.scope, instance)
                    && fact
                        .expires_at_unix_ms
                        .is_none_or(|expiry| time.unix_ms <= expiry)
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            activity_scope_specificity(&right.scope)
                .cmp(&activity_scope_specificity(&left.scope))
                .then_with(|| right.observed_at_unix_ms.cmp(&left.observed_at_unix_ms))
        });
        let target = match records.first() {
            None => ChoiceRows::Absent,
            Some(record) => match &record.value {
                FactValue::RecordList(rows) => rows
                    .iter()
                    .map(RemedyChoiceRow::from_fact_row)
                    .collect::<Option<Vec<_>>>()
                    .map_or(ChoiceRows::Unreadable, ChoiceRows::Rows),
                _ => ChoiceRows::Unreadable,
            },
        };
        let configuration_choice = if configuration.is_empty() {
            ChoiceRows::Absent
        } else if configuration.iter().all(RemedyChoiceRow::is_token_shaped) {
            ChoiceRows::Rows(configuration.to_vec())
        } else {
            ChoiceRows::Unreadable
        };
        Self {
            target,
            records,
            configuration: configuration_choice,
            configuration_rows: configuration,
        }
    }

    fn sources(&self) -> [(RemedySource, &ChoiceRows); 2] {
        [
            (RemedySource::Target, &self.target),
            (RemedySource::Configuration, &self.configuration),
        ]
    }
}

/// The allowed set of a need on an instance (model §5.3 step 1).
enum Allowed {
    Set {
        capabilities: Vec<String>,
        source: RemedySource,
    },
    Stop {
        problem: RemedyProblem,
        source: Option<RemedySource>,
        capability: Option<String>,
    },
}

fn resolve_allowed(remedy: &RemedyTableRemedy, choices: &InstanceChoices<'_>) -> Allowed {
    for (source, rows) in choices.sources() {
        let rows = match rows {
            ChoiceRows::Absent => continue,
            ChoiceRows::Unreadable => {
                return Allowed::Stop {
                    problem: RemedyProblem::ChoiceInvalid,
                    source: Some(source),
                    capability: None,
                };
            }
            ChoiceRows::Rows(rows) => rows,
        };
        let named = rows
            .iter()
            .filter(|row| row.need() == Some(remedy.need.as_str()))
            .collect::<Vec<_>>();
        if named.is_empty() {
            continue;
        }
        if named
            .iter()
            .any(|row| matches!(row, RemedyChoiceRow::NoRemedy { .. }))
        {
            return Allowed::Stop {
                problem: RemedyProblem::ChoiceNone,
                source: Some(source),
                capability: None,
            };
        }
        let decided = named
            .iter()
            .filter_map(|row| match row {
                RemedyChoiceRow::Allow { capability, .. } => Some(capability.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        return intersect(remedy, &decided, source);
    }
    if !remedy.default.is_empty() {
        let decided = remedy
            .default
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        return intersect(remedy, &decided, RemedySource::Package);
    }
    Allowed::Stop {
        problem: RemedyProblem::NoChoice,
        source: None,
        capability: None,
    }
}

/// The decided set intersected with the declared capabilities, in declaration order; a decided
/// capability the remedy does not declare, or nothing left, is `choice_invalid`.
fn intersect(remedy: &RemedyTableRemedy, decided: &[&str], source: RemedySource) -> Allowed {
    if let Some(outside) = decided.iter().find(|capability| {
        !remedy
            .capabilities
            .iter()
            .any(|declared| declared == *capability)
    }) {
        return Allowed::Stop {
            problem: RemedyProblem::ChoiceInvalid,
            source: Some(source),
            capability: Some((*outside).to_owned()),
        };
    }
    let capabilities = remedy
        .capabilities
        .iter()
        .filter(|declared| decided.contains(&declared.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if capabilities.is_empty() {
        return Allowed::Stop {
            problem: RemedyProblem::ChoiceInvalid,
            source: Some(source),
            capability: None,
        };
    }
    Allowed::Set {
        capabilities,
        source,
    }
}

/// One resolved parameter value of a provider's package (model §5.3 step 2).
struct ResolvedParameter {
    name: String,
    value: String,
    source: RemedySource,
}

/// A parameter choice the provider does not declare, or a value outside its options.
struct ParameterProblem {
    source: RemedySource,
    capability: String,
}

/// Every declared parameter of a package's `provides`, each by the first source that sets it:
/// the target record, the configuration, the declared default. A row of either source naming a
/// parameter the capability does not declare, two values in one source, an unreadable source or
/// a value outside the options refuses the provider.
fn resolve_parameters(
    provides: &[RemedyTableCapability],
    choices: &InstanceChoices<'_>,
) -> Result<Vec<ResolvedParameter>, ParameterProblem> {
    let mut resolved = Vec::new();
    for provided in provides {
        let problem = |source| ParameterProblem {
            source,
            capability: provided.capability.clone(),
        };
        for (source, rows) in choices.sources() {
            match rows {
                ChoiceRows::Absent => {}
                ChoiceRows::Unreadable if !provided.parameters.is_empty() => {
                    return Err(problem(source));
                }
                ChoiceRows::Unreadable => {}
                ChoiceRows::Rows(rows) => {
                    let undeclared = rows.iter().any(|row| {
                        matches!(row, RemedyChoiceRow::Parameter { capability, parameter, .. }
                            if *capability == provided.capability
                                && !provided.parameters.iter().any(|declared| declared.name == *parameter))
                    });
                    if undeclared {
                        return Err(problem(source));
                    }
                }
            }
        }
        for parameter in &provided.parameters {
            let mut chosen = None;
            for (source, rows) in choices.sources() {
                let values = rows.parameter_values(&provided.capability, &parameter.name);
                match values.as_slice() {
                    [] => continue,
                    [value] => {
                        chosen = Some(((*value).to_owned(), source));
                        break;
                    }
                    _ => return Err(problem(source)),
                }
            }
            let (value, source) =
                chosen.unwrap_or_else(|| (parameter.default.clone(), RemedySource::Package));
            if !parameter.options.contains(&value) {
                return Err(problem(source));
            }
            resolved.push(ResolvedParameter {
                name: parameter.name.clone(),
                value,
                source,
            });
        }
    }
    Ok(resolved)
}

/// One run of an instance, with the task it ran.
#[derive(Clone, Copy)]
struct InstanceRun<'a> {
    task_id: &'a str,
    run: &'a RemedyRun,
}

/// One layer of a round's chain (model §7.1).
struct Layer<'a> {
    task_id: &'a str,
    remedy: &'a RemedyTableRemedy,
    last_blocked_at_unix_ms: u64,
    requests_used: u32,
    served_by: Option<String>,
    /// The capability its task was requested for in the layer below (layers above the first).
    requested_for: Option<String>,
}

impl<'a> Layer<'a> {
    fn new(
        task_id: &'a str,
        remedy: &'a RemedyTableRemedy,
        blocked_at: u64,
        requested_for: Option<String>,
    ) -> Self {
        Self {
            task_id,
            remedy,
            last_blocked_at_unix_ms: blocked_at,
            requests_used: 0,
            served_by: None,
            requested_for,
        }
    }
}

enum RoundEnd {
    Resolved,
    /// The retry failed, or the root ran again outside the round: no code.
    Closed,
    Exhausted {
        problem: RemedyProblem,
        index: usize,
    },
}

/// One round, folded from the instance's runs.
struct Round<'a> {
    root_task: &'a str,
    decision_id: &'a str,
    hex: String,
    digest: String,
    window_end_unix_ms: u64,
    layers: Vec<Layer<'a>>,
    in_flight: bool,
    end: Option<RoundEnd>,
}

impl<'a> Round<'a> {
    /// The layer at `index` is served by `capability`; the layers above it are left.
    fn serve(&mut self, index: usize, capability: String) {
        if self.layers[index].served_by.is_none() {
            self.layers[index].served_by = Some(capability);
            self.layers.truncate(index + 1);
        }
    }

    fn chain(&self) -> Vec<&'a str> {
        self.layers.iter().map(|layer| layer.task_id).collect()
    }
}

/// What a requesting layer can request.
enum Requestable<'a> {
    Providers(Vec<(&'a str, String)>),
    Wait,
    Stop {
        code: PolicyCode,
        problem: RemedyProblem,
        source: Option<RemedySource>,
        capability: Option<String>,
    },
}

struct InstanceContext<'a> {
    catalog: &'a CompiledCatalog,
    instance: &'a InstanceSnapshot,
    choices: &'a InstanceChoices<'a>,
    table: &'a TableIndex<'a>,
    inputs: &'a RemedyInputs,
    time: EvaluationTime,
}

impl<'a> InstanceContext<'a> {
    fn instance_id(&self) -> &'a str {
        &self.instance.instance_id
    }

    /// The catalog tasks whose scope covers the instance, in catalog order.
    fn tasks(&self) -> impl Iterator<Item = &'a TaskSpec> {
        let instance = self.instance;
        self.catalog
            .catalog()
            .tasks
            .tasks
            .iter()
            .filter(move |task| scope_matches_instance(&task.scope, instance))
    }

    fn covers(&self, task_id: &str) -> bool {
        self.tasks().any(|task| task.id == task_id)
    }

    /// The catalog tasks on the instance whose package provides `capability`.
    fn providers_of(&self, capability: &str) -> Vec<(&'a str, &'a RemedyTableCapability)> {
        let table = self.table;
        self.tasks()
            .filter_map(|task| {
                table
                    .provided(&task.id, capability)
                    .map(|provided| (task.id.as_str(), provided))
            })
            .collect()
    }

    fn is_blocked_key(&self, task_id: &str, run: &RemedyRun) -> Option<&'a RemedyTableRemedy> {
        match &run.settled {
            RemedySettlement::Succeeded {
                outcome_key: Some(key),
            } => self.table.remedy(task_id, key),
            _ => None,
        }
    }

    /// A run that succeeded on a remedy key and was neither a request nor a retry.
    fn root_remedy(&self, task_id: &str, run: &RemedyRun) -> Option<&'a RemedyTableRemedy> {
        let remedy_run = run.remedy_reasons.iter().any(|outcome| {
            outcome.is(PolicyCode::PolicyRemedyRequested)
                || outcome.is(PolicyCode::PolicyRemedyRetry)
        });
        if remedy_run {
            None
        } else {
            self.is_blocked_key(task_id, run)
        }
    }

    fn plan_rounds(&self, plan: &mut RemedyPlan) {
        let instance_id = self.instance_id();
        let journals = self
            .inputs
            .journal
            .iter()
            .filter(|journal| journal.instance_id == instance_id && self.covers(&journal.task_id))
            .collect::<Vec<_>>();
        let mut runs = journals
            .iter()
            .copied()
            .flat_map(|journal| {
                journal.runs.iter().map(move |run| InstanceRun {
                    task_id: journal.task_id.as_str(),
                    run,
                })
            })
            .collect::<Vec<_>>();
        runs.sort_by(|left, right| {
            left.run
                .admitted_at_unix_ms
                .cmp(&right.run.admitted_at_unix_ms)
                .then_with(|| left.task_id.cmp(right.task_id))
                .then_with(|| left.run.decision_id.cmp(&right.run.decision_id))
        });
        let stops = self
            .inputs
            .stops
            .iter()
            .filter(|stop| stop.instance_id == instance_id)
            .collect::<Vec<_>>();
        let mut handled = BTreeSet::new();
        let mut open = Vec::new();
        let mut held = BTreeSet::new();
        for journal in journals.iter().copied() {
            let task_id = journal.task_id.as_str();
            let Some((root, remedy)) = journal
                .runs
                .iter()
                .rev()
                .find_map(|run| self.root_remedy(task_id, run).map(|remedy| (run, remedy)))
            else {
                continue;
            };
            let hex = round_hex(task_id, instance_id, &root.decision_id);
            let stop = stops
                .iter()
                .enumerate()
                .find(|(_, stop)| stop.task_id == task_id && stop.decision_id == root.decision_id);
            if let Some((index, _)) = stop {
                handled.insert(index);
            }
            if self.seen("lifted", &hex) || self.seen("expired", &hex) {
                continue;
            }
            let digest = self.inputs_digest(task_id, Some(remedy.outcome_key.as_str()));
            if let Some((_, stop)) = stop {
                if stop.remedy_inputs_digest == digest {
                    held.insert(task_id);
                    plan.deferrals.insert(
                        (task_id.to_owned(), instance_id.to_owned()),
                        RemedyDeferral {
                            reason: outcome_reason(stop.outcome.clone()),
                            next_wake_unix_ms: None,
                        },
                    );
                } else {
                    self.lift(plan, task_id, &root.decision_id, &hex, digest);
                }
                continue;
            }
            let round = self.fold(task_id, root, remedy, hex, digest, &runs);
            if matches!(round.end, Some(RoundEnd::Resolved | RoundEnd::Closed)) {
                continue;
            }
            if self.time.unix_ms >= round.window_end_unix_ms {
                self.expire(plan, &round);
                continue;
            }
            held.extend(round.chain());
            open.push(round);
        }
        // A recorded stop whose round is no longer the pair's latest is lifted: its inputs (the
        // pair's root blocked run, or the key's remedy) are gone.
        for (index, stop) in stops.iter().enumerate() {
            let hex = round_hex(&stop.task_id, instance_id, &stop.decision_id);
            if handled.contains(&index) || self.seen("lifted", &hex) {
                continue;
            }
            let digest = self.inputs_digest(&stop.task_id, None);
            self.lift(plan, &stop.task_id, &stop.decision_id, &hex, digest);
        }
        for round in &open {
            self.plan_round(plan, round, &held);
        }
    }

    fn seen(&self, kind: &str, hex: &str) -> bool {
        self.inputs
            .signals_seen
            .contains(&format!("signal:remedy:{kind}:{hex}"))
    }

    /// Folds one round from the instance's runs after its root blocked run (model §7.4).
    fn fold(
        &self,
        root_task: &'a str,
        root: &'a RemedyRun,
        remedy: &'a RemedyTableRemedy,
        hex: String,
        digest: String,
        runs: &[InstanceRun<'a>],
    ) -> Round<'a> {
        let decision_id = root.decision_id.as_str();
        let mut round = Round {
            root_task,
            decision_id,
            hex,
            digest,
            window_end_unix_ms: root.activity_window_end_unix_ms,
            layers: vec![Layer::new(
                root_task,
                remedy,
                root.admitted_at_unix_ms,
                None,
            )],
            in_flight: false,
            end: None,
        };
        let max_per_round = u32::from(self.inputs.max_per_round);
        for entry in runs
            .iter()
            .filter(|entry| entry.run.admitted_at_unix_ms > root.admitted_at_unix_ms)
        {
            if round.end.is_some() {
                break;
            }
            let reasons = entry
                .run
                .remedy_reasons
                .iter()
                .filter(|outcome| text_value(outcome, "decision_id") == Some(decision_id))
                .collect::<Vec<_>>();
            let retry = reasons
                .iter()
                .filter(|outcome| outcome.is(PolicyCode::PolicyRemedyRetry))
                .find_map(|outcome| layer_of(&round, outcome))
                .filter(|index| round.layers[*index].task_id == entry.task_id);
            if let Some(index) = retry {
                self.on_retry(&mut round, index, entry, max_per_round);
                continue;
            }
            let request = reasons
                .iter()
                .filter(|outcome| outcome.is(PolicyCode::PolicyRemedyRequested))
                .find_map(|outcome| {
                    layer_of(&round, outcome)
                        .map(|index| (index, text_value(outcome, "capability").map(str::to_owned)))
                });
            if let Some((index, capability)) = request {
                let capability = capability.unwrap_or_else(|| {
                    self.serving_capability(&round.layers[index], entry.task_id)
                });
                self.on_request(&mut round, index, entry, capability);
                continue;
            }
            self.on_other_run(&mut round, entry);
        }
        round
    }

    /// The capability by which `task_id` serves `layer`: the first allowed capability it
    /// provides, else the first declared one it provides, else the layer's first declared one.
    fn serving_capability(&self, layer: &Layer<'a>, task_id: &str) -> String {
        let allowed = match resolve_allowed(layer.remedy, self.choices) {
            Allowed::Set { capabilities, .. } => capabilities,
            Allowed::Stop { .. } => Vec::new(),
        };
        allowed
            .iter()
            .chain(&layer.remedy.capabilities)
            .find(|capability| self.table.provided(task_id, capability).is_some())
            .or_else(|| layer.remedy.capabilities.first())
            .cloned()
            .unwrap_or_default()
    }

    fn on_retry(&self, round: &mut Round<'a>, index: usize, entry: &InstanceRun<'a>, max: u32) {
        match &entry.run.settled {
            RemedySettlement::InFlight => round.in_flight = true,
            RemedySettlement::Failed => round.end = Some(RoundEnd::Closed),
            RemedySettlement::Succeeded { .. } => {
                match self.is_blocked_key(entry.task_id, entry.run) {
                    // Blocked again: a new cycle of the same layer, unless its requests are used.
                    Some(remedy) => {
                        round.layers.truncate(index + 1);
                        let layer = &mut round.layers[index];
                        layer.remedy = remedy;
                        layer.last_blocked_at_unix_ms = entry.run.admitted_at_unix_ms;
                        layer.served_by = None;
                        if layer.requests_used >= max {
                            round.end = Some(RoundEnd::Exhausted {
                                problem: RemedyProblem::RoundLimit,
                                index,
                            });
                        }
                    }
                    // Resolved: the layer below is served by this run.
                    None if index == 0 => {
                        round.layers.clear();
                        round.end = Some(RoundEnd::Resolved);
                    }
                    None => {
                        let capability = match round.layers[index].requested_for.clone() {
                            Some(capability) => capability,
                            None => {
                                self.serving_capability(&round.layers[index - 1], entry.task_id)
                            }
                        };
                        round.layers.truncate(index);
                        round.serve(index - 1, capability);
                    }
                }
            }
        }
    }

    fn on_request(
        &self,
        round: &mut Round<'a>,
        index: usize,
        entry: &InstanceRun<'a>,
        capability: String,
    ) {
        round.layers[index].requests_used += 1;
        match &entry.run.settled {
            RemedySettlement::InFlight => round.in_flight = true,
            // The request is used; the layer may request again.
            RemedySettlement::Failed => {}
            RemedySettlement::Succeeded { .. } => {
                match self.is_blocked_key(entry.task_id, entry.run) {
                    Some(remedy) => {
                        round.layers.truncate(index + 1);
                        if index + 1 >= REMEDY_MAX_DEPTH {
                            round.end = Some(RoundEnd::Exhausted {
                                problem: RemedyProblem::DepthLimit,
                                index,
                            });
                        } else {
                            round.layers.push(Layer::new(
                                entry.task_id,
                                remedy,
                                entry.run.admitted_at_unix_ms,
                                Some(capability),
                            ));
                        }
                    }
                    None => round.serve(index, capability),
                }
            }
        }
    }

    /// A run that carries no reason of the round: the root's own run ends the round; any other
    /// successful run of a provider of a layer's allowed set, admitted after the layer's last
    /// blocked run, serves the lowest such layer, requested or not.
    fn on_other_run(&self, round: &mut Round<'a>, entry: &InstanceRun<'a>) {
        let blocked = self.is_blocked_key(entry.task_id, entry.run).is_some();
        if entry.task_id == round.root_task {
            match &entry.run.settled {
                RemedySettlement::InFlight => round.in_flight = true,
                RemedySettlement::Failed => round.end = Some(RoundEnd::Closed),
                RemedySettlement::Succeeded { .. } if !blocked => {
                    round.end = Some(RoundEnd::Resolved);
                }
                RemedySettlement::Succeeded { .. } => {}
            }
            return;
        }
        if !matches!(entry.run.settled, RemedySettlement::Succeeded { .. }) || blocked {
            return;
        }
        let served = round.layers.iter().enumerate().find_map(|(index, layer)| {
            if layer.served_by.is_some()
                || layer.task_id == entry.task_id
                || entry.run.admitted_at_unix_ms <= layer.last_blocked_at_unix_ms
            {
                return None;
            }
            let Allowed::Set { capabilities, .. } = resolve_allowed(layer.remedy, self.choices)
            else {
                return None;
            };
            capabilities
                .into_iter()
                .find(|capability| self.table.provided(entry.task_id, capability).is_some())
                .map(|capability| (index, capability))
        });
        if let Some((index, capability)) = served {
            round.serve(index, capability);
        }
    }

    /// The plan of one open round: a stop, the retry of a served layer, a wait for a run in
    /// flight, or the requests of the top layer; every blocked pair of the chain is held but the
    /// one retried.
    fn plan_round(&self, plan: &mut RemedyPlan, round: &Round<'a>, held: &BTreeSet<&'a str>) {
        let max_per_round = self.inputs.max_per_round;
        if let Some(RoundEnd::Exhausted { problem, index }) = round.end {
            self.stop_exhausted(plan, round, index, problem);
            return;
        }
        let top_index = round.layers.len() - 1;
        let top = &round.layers[top_index];
        if round.in_flight {
            self.hold(plan, round, None);
            return;
        }
        if let Some(capability) = &top.served_by {
            let outcome = Outcome::new(PolicyCode::PolicyRemedyRetry)
                .task_id(top.task_id)
                .instance_id(self.instance_id())
                .decision_id(round.decision_id)
                .outcome_key(top.remedy.outcome_key.as_str())
                .capability(capability.as_str())
                .remedy_depth(depth(top_index))
                .count(top.requests_used)
                .limit_count(max_per_round);
            plan.overrides.insert(
                (top.task_id.to_owned(), self.instance_id().to_owned()),
                RemedyOverride::Retry {
                    reason: outcome_reason(outcome),
                },
            );
            self.hold(plan, round, Some(top.task_id));
            return;
        }
        if top.requests_used >= u32::from(max_per_round) {
            self.stop_exhausted(plan, round, top_index, RemedyProblem::RoundLimit);
            return;
        }
        let (capabilities, source) = match resolve_allowed(top.remedy, self.choices) {
            Allowed::Set {
                capabilities,
                source,
            } => (capabilities, source),
            Allowed::Stop {
                problem,
                source,
                capability,
            } => {
                self.stop(
                    plan,
                    round,
                    top_index,
                    PolicyCode::PolicyRemedyUnchosen,
                    problem,
                    source,
                    capability,
                );
                return;
            }
        };
        match self.requestable(round, top, &capabilities, held) {
            Requestable::Providers(providers) => {
                self.request(plan, round, top_index, &capabilities, source, &providers);
                self.hold(plan, round, None);
            }
            Requestable::Wait => self.hold(plan, round, None),
            Requestable::Stop {
                code,
                problem,
                source: problem_source,
                capability,
            } => self.stop(
                plan,
                round,
                top_index,
                code,
                problem,
                problem_source.or(Some(source)),
                capability,
            ),
        }
    }

    /// The providers of the allowed set the top layer can request (model §7.3): a catalog task
    /// on the instance that provides an allowed capability, is not a blocked pair of the chain,
    /// is not on a refused edge, agrees on its parameters with the capability's other providers,
    /// and resolves its parameters. A provider that is held by another round only waits.
    fn requestable(
        &self,
        round: &Round<'a>,
        layer: &Layer<'a>,
        capabilities: &[String],
        held: &BTreeSet<&'a str>,
    ) -> Requestable<'a> {
        let chain = round.chain();
        let mut requested: Vec<(&'a str, String)> = Vec::new();
        let mut any_provider = false;
        let mut excluded = false;
        let mut mismatch = false;
        let mut waiting = false;
        let mut parameter_problem = None;
        for capability in capabilities {
            let providers = self.providers_of(capability);
            if providers.is_empty() {
                continue;
            }
            any_provider = true;
            let eligible = providers
                .iter()
                .filter(|(task_id, _)| {
                    !chain.contains(task_id)
                        && !self.table.refused.contains(&(layer.task_id, *task_id))
                })
                .collect::<Vec<_>>();
            if eligible.is_empty() {
                excluded = true;
                continue;
            }
            let declarations = providers
                .iter()
                .map(|(_, provided)| normalized_parameters(provided))
                .collect::<BTreeSet<_>>();
            if declarations.len() > 1 {
                mismatch = true;
                continue;
            }
            for &&(task_id, _) in &eligible {
                if requested.iter().any(|(requested, _)| *requested == task_id) {
                    continue;
                }
                let provides = self.table.provides.get(task_id).copied().unwrap_or(&[]);
                if let Err(problem) = resolve_parameters(provides, self.choices) {
                    if parameter_problem.is_none() {
                        parameter_problem = Some(problem);
                    }
                    continue;
                }
                if held.contains(task_id) {
                    waiting = true;
                    continue;
                }
                requested.push((task_id, capability.clone()));
            }
        }
        if !requested.is_empty() {
            return Requestable::Providers(requested);
        }
        if waiting {
            return Requestable::Wait;
        }
        if let Some(problem) = parameter_problem {
            return Requestable::Stop {
                code: PolicyCode::PolicyRemedyUnchosen,
                problem: RemedyProblem::ParameterInvalid,
                source: Some(problem.source),
                capability: Some(problem.capability),
            };
        }
        let problem = if mismatch {
            RemedyProblem::ParametersMismatch
        } else if excluded || any_provider {
            RemedyProblem::Cycle
        } else {
            layer
                .remedy
                .problem
                .unwrap_or(RemedyProblem::CapabilityUnbound)
        };
        Requestable::Stop {
            code: PolicyCode::PolicyRemedyUnavailable,
            problem,
            source: None,
            capability: None,
        }
    }

    /// One `policy_remedy_requested` per provider for the top layer, merged into the provider's
    /// override; the layer is remembered for the persistent/transient split.
    fn request(
        &self,
        plan: &mut RemedyPlan,
        round: &Round<'a>,
        index: usize,
        capabilities: &[String],
        source: RemedySource,
        providers: &[(&'a str, String)],
    ) {
        let layer = &round.layers[index];
        let instance_id = self.instance_id();
        for (provider, capability) in providers {
            let irreversible = self
                .providers_of(capability)
                .iter()
                .any(|(_, provided)| provided.irreversible);
            let outcome = Outcome::new(PolicyCode::PolicyRemedyRequested)
                .task_id(layer.task_id)
                .instance_id(instance_id)
                .decision_id(round.decision_id)
                .outcome_key(layer.remedy.outcome_key.as_str())
                .need(layer.remedy.need.as_str())
                .capability(capability.as_str())
                .capabilities_allowed(capabilities.iter().cloned())
                .capabilities_declared(layer.remedy.capabilities.iter().cloned())
                .remedy_source(source)
                .remedy_depth(depth(index))
                .count(layer.requests_used + 1)
                .limit_count(self.inputs.max_per_round)
                .irreversible(irreversible);
            let entry = plan
                .overrides
                .entry(((*provider).to_owned(), instance_id.to_owned()))
                .or_insert_with(|| RemedyOverride::Request {
                    reasons: Vec::new(),
                    promoted: true,
                });
            if let RemedyOverride::Request { reasons, .. } = entry
                && reasons.len() < REMEDY_MAX_REQUESTED_REASONS
            {
                reasons.push(outcome_reason(outcome));
            }
        }
        let stop = self.stop_outcome(
            round,
            index,
            PolicyCode::PolicyRemedyUnavailable,
            RemedyProblem::ProviderUnavailable,
            Some(source),
            None,
        );
        plan.requests.push(LayerRequest {
            instance_id: instance_id.to_owned(),
            root_task_id: round.root_task.to_owned(),
            blocked_task_id: layer.task_id.to_owned(),
            providers: providers
                .iter()
                .map(|(provider, _)| (*provider).to_owned())
                .collect(),
            hold: self.waiting_outcome(round, layer),
            stop,
            signal_id: format!("signal:remedy:stopped:{}", round.hex),
        });
    }

    /// Holds every blocked pair of the chain but `retried`, waking at the round's window end.
    fn hold(&self, plan: &mut RemedyPlan, round: &Round<'a>, retried: Option<&str>) {
        for layer in &round.layers {
            if Some(layer.task_id) == retried {
                continue;
            }
            plan.deferrals
                .entry((layer.task_id.to_owned(), self.instance_id().to_owned()))
                .or_insert_with(|| RemedyDeferral {
                    reason: outcome_reason(self.waiting_outcome(round, layer)),
                    next_wake_unix_ms: Some(round.window_end_unix_ms),
                });
        }
    }

    fn waiting_outcome(&self, round: &Round<'a>, layer: &Layer<'a>) -> Outcome {
        Outcome::new(PolicyCode::PolicyRemedyWaiting)
            .task_id(layer.task_id)
            .instance_id(self.instance_id())
            .outcome_key(layer.remedy.outcome_key.as_str())
            .need(layer.remedy.need.as_str())
            .decision_id(round.decision_id)
    }

    /// The stop outcome of a round at layer `index`: the root task, the layer's key and need.
    fn stop_outcome(
        &self,
        round: &Round<'a>,
        index: usize,
        code: PolicyCode,
        problem: RemedyProblem,
        source: Option<RemedySource>,
        capability: Option<String>,
    ) -> Outcome {
        let layer = &round.layers[index];
        let mut outcome = Outcome::new(code)
            .task_id(round.root_task)
            .instance_id(self.instance_id())
            .decision_id(round.decision_id)
            .outcome_key(layer.remedy.outcome_key.as_str())
            .need(layer.remedy.need.as_str())
            .remedy_problem(problem)
            .remedy_inputs_digest(round.digest.as_str());
        if let Some(source) = source {
            outcome = outcome.remedy_source(source);
        }
        if let Some(capability) = capability {
            outcome = outcome.capability(capability);
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn stop(
        &self,
        plan: &mut RemedyPlan,
        round: &Round<'a>,
        index: usize,
        code: PolicyCode,
        problem: RemedyProblem,
        source: Option<RemedySource>,
        capability: Option<String>,
    ) {
        let outcome = self.stop_outcome(round, index, code, problem, source, capability);
        self.record_stop(plan, round, outcome);
    }

    fn stop_exhausted(
        &self,
        plan: &mut RemedyPlan,
        round: &Round<'a>,
        index: usize,
        problem: RemedyProblem,
    ) {
        let layer = &round.layers[index];
        let outcome = self
            .stop_outcome(
                round,
                index,
                PolicyCode::PolicyRemedyExhausted,
                problem,
                None,
                None,
            )
            .count(layer.requests_used)
            .limit_count(self.inputs.max_per_round)
            .remedy_depth(depth(index));
        self.record_stop(plan, round, outcome);
    }

    /// The root stays deferred with the stop, no wake; the chain's other pairs return to their
    /// triggers. The stop is signalled once per round.
    fn record_stop(&self, plan: &mut RemedyPlan, round: &Round<'a>, outcome: Outcome) {
        let instance_id = self.instance_id();
        plan.deferrals.insert(
            (round.root_task.to_owned(), instance_id.to_owned()),
            RemedyDeferral {
                reason: outcome_reason(outcome.clone()),
                next_wake_unix_ms: None,
            },
        );
        plan.push_signal(
            format!("signal:remedy:stopped:{}", round.hex),
            round.root_task,
            instance_id,
            outcome,
        );
    }

    fn lift(
        &self,
        plan: &mut RemedyPlan,
        task_id: &str,
        decision_id: &str,
        hex: &str,
        digest: String,
    ) {
        let instance_id = self.instance_id();
        let outcome = Outcome::new(PolicyCode::PolicyRemedyLifted)
            .task_id(task_id)
            .instance_id(instance_id)
            .decision_id(decision_id)
            .remedy_inputs_digest(digest);
        plan.push_signal(
            format!("signal:remedy:lifted:{hex}"),
            task_id,
            instance_id,
            outcome,
        );
    }

    fn expire(&self, plan: &mut RemedyPlan, round: &Round<'a>) {
        let Some(layer) = round.layers.last() else {
            return;
        };
        let outcome = Outcome::new(PolicyCode::PolicyRemedyExpired)
            .task_id(round.root_task)
            .instance_id(self.instance_id())
            .decision_id(round.decision_id)
            .outcome_key(layer.remedy.outcome_key.as_str())
            .need(layer.remedy.need.as_str())
            .window_end_unix_ms(round.window_end_unix_ms);
        plan.push_signal(
            format!("signal:remedy:expired:{}", round.hex),
            round.root_task,
            self.instance_id(),
            outcome,
        );
    }

    /// `policy_dispatch_parameters` for every pair whose package declares parameters, when each
    /// value resolves to one of its options (model §6): a pair that does not resolve carries
    /// none, and is not requestable either.
    fn plan_run_parameters(&self, plan: &mut RemedyPlan) {
        for task in self.tasks() {
            let Some(provides) = self.table.provides.get(task.id.as_str()) else {
                continue;
            };
            if provides
                .iter()
                .all(|provided| provided.parameters.is_empty())
            {
                continue;
            }
            let Ok(parameters) = resolve_parameters(provides, self.choices) else {
                continue;
            };
            let records = parameters.iter().map(|parameter| {
                Record::new()
                    .parameter_name(parameter.name.as_str())
                    .parameter_value(parameter.value.as_str())
                    .remedy_source(parameter.source)
            });
            let outcome =
                Outcome::new(PolicyCode::PolicyDispatchParameters).run_parameters(records);
            plan.run_parameters.insert(
                (task.id.clone(), self.instance_id().to_owned()),
                outcome_reason(outcome),
            );
        }
    }

    /// `remedy_inputs_digest` (model §7.6): the versions of the target records that apply to the
    /// instance, the configuration rows in effect, the remedy of the root's key, every provider
    /// of its capabilities on the instance with its declarations, the edges refused from the root
    /// task, and the catalog hash. Declarations and versions only, never a package's content.
    fn inputs_digest(&self, root_task: &str, root_key: Option<&str>) -> String {
        let mut hasher = Sha256::new();
        hash_part(&mut hasher, b"remedy-inputs");
        hash_part(&mut hasher, b"target");
        for record in &self.choices.records {
            let (kind, id) = match &record.scope {
                ScopeSelector::Instance { instance_id } => ("instance", instance_id),
                ScopeSelector::Server { server_id } => ("server", server_id),
                ScopeSelector::Game { game_id } => ("game", game_id),
            };
            hash_part(&mut hasher, kind.as_bytes());
            hash_part(&mut hasher, id.as_bytes());
            hasher.update(record.observed_at_unix_ms.to_be_bytes());
            hasher.update(record.expires_at_unix_ms.unwrap_or(u64::MAX).to_be_bytes());
            if let FactValue::RecordList(rows) = &record.value {
                for row in rows {
                    for (field, value) in row {
                        hash_part(&mut hasher, field.as_bytes());
                        match value {
                            FactScalar::String(text) => hash_part(&mut hasher, text.as_bytes()),
                            other => hash_part(&mut hasher, format!("{other:?}").as_bytes()),
                        }
                    }
                }
            }
        }
        hash_part(&mut hasher, b"configuration");
        for row in self.choices.configuration_rows {
            row.hash_into(&mut hasher);
        }
        hash_part(&mut hasher, b"remedy");
        let remedy = root_key.and_then(|key| self.table.remedy(root_task, key));
        match remedy {
            None => hash_part(&mut hasher, b"absent"),
            Some(remedy) => {
                for part in [&remedy.task_id, &remedy.outcome_key, &remedy.need] {
                    hash_part(&mut hasher, part.as_bytes());
                }
                hash_list(&mut hasher, &remedy.capabilities);
                hash_list(&mut hasher, &remedy.default);
                hash_part(
                    &mut hasher,
                    remedy.problem.map_or("", RemedyProblem::as_str).as_bytes(),
                );
                hash_part(&mut hasher, b"providers");
                for capability in &remedy.capabilities {
                    hash_part(&mut hasher, capability.as_bytes());
                    for (task_id, provided) in self.providers_of(capability) {
                        hash_part(&mut hasher, task_id.as_bytes());
                        hasher.update([u8::from(provided.irreversible)]);
                        for parameter in &provided.parameters {
                            hash_part(&mut hasher, parameter.name.as_bytes());
                            hash_list(&mut hasher, &parameter.options);
                            hash_part(&mut hasher, parameter.default.as_bytes());
                        }
                    }
                }
            }
        }
        hash_part(&mut hasher, b"refused");
        let refused = self
            .table
            .refused
            .iter()
            .filter(|(blocked, _)| *blocked == root_task)
            .map(|(_, provider)| *provider)
            .collect::<BTreeSet<_>>();
        for provider in refused {
            hash_part(&mut hasher, provider.as_bytes());
        }
        hash_part(&mut hasher, b"catalog");
        hash_part(&mut hasher, self.catalog.catalog_hash().as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    }
}

/// The layer a remedy reason names: its `remedy_depth`, whose blocked task is its `task_id`.
fn layer_of(round: &Round<'_>, outcome: &Outcome) -> Option<usize> {
    let depth = outcome
        .value("remedy_depth")
        .and_then(serde_json::Value::as_u64)?;
    let index = usize::try_from(depth).ok()?.checked_sub(1)?;
    let layer = round.layers.get(index)?;
    (text_value(outcome, "task_id") == Some(layer.task_id)).then_some(index)
}

fn text_value<'o>(outcome: &'o Outcome, key: &str) -> Option<&'o str> {
    outcome.value(key).and_then(serde_json::Value::as_str)
}

/// `remedy_depth` of the layer at `index`.
fn depth(index: usize) -> u8 {
    u8::try_from(index + 1).unwrap_or(u8::MAX)
}

/// A provider's parameter declarations, compared as a set (model §9.1, `parameters_mismatch`).
fn normalized_parameters(
    provided: &RemedyTableCapability,
) -> BTreeSet<(&str, BTreeSet<&str>, &str)> {
    provided
        .parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.as_str(),
                parameter.options.iter().map(String::as_str).collect(),
                parameter.default.as_str(),
            )
        })
        .collect()
}

/// `<hex>` of a round's signal ids: the lowercase SHA-256 of `task \0 instance \0 decision_id`.
fn round_hex(task_id: &str, instance_id: &str, decision_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(task_id.as_bytes());
    hasher.update([0_u8]);
    hasher.update(instance_id.as_bytes());
    hasher.update([0_u8]);
    hasher.update(decision_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hash_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hash_list(hasher: &mut Sha256, items: &[String]) {
    hasher.update((items.len() as u64).to_be_bytes());
    for item in items {
        hash_part(hasher, item.as_bytes());
    }
}

/// A reason of the plan: its outcome, with the outcome as its developer detail.
fn outcome_reason(outcome: Outcome) -> DecisionReason {
    DecisionReason {
        detail: outcome.to_string(),
        outcome,
    }
}
