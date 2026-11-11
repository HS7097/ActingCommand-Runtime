// SPDX-License-Identifier: AGPL-3.0-only

//! The evaluator: one pure function from declared inputs to one explained decision.
//!
//! [`evaluate`] validates the document, hashes the document and the inputs, runs the hard
//! gates and the scoring terms over every candidate in document order, ranks the survivors,
//! applies the declared tie-break keys, and reports what it chose together with a complete
//! reason chain. It reads no clock, no ledger, no pack, and no file: `now_unix_ms` is an
//! argument, so the same inputs always produce the same decision and the same hashes.
//!
//! Three-valued logic runs through the whole pass. A gate whose predicate cannot be decided,
//! or a term whose value is unknown, applies the handling its document declares and records
//! that it did. A tie-break key whose value is unknown orders its candidate behind every
//! candidate whose value is known, in either direction, and the candidate's verdict says so.
//! Nothing is silently read as `false` or `0`.
//!
//! [`evaluate_collect`] is the collect option, the call a scheduling layer makes: the same
//! gates and terms over the same inputs, but an unknown input is listed per candidate instead
//! of being handled by the document's clauses, and nothing is selected.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Outcome, VocabularyToken};
use serde::{Deserialize, Serialize};

use crate::canonical::canonical_sha256;
use crate::codes::SelectionPolicyCode;
use crate::facts::{Candidate, RecordRow, ScalarValue, SelectionFactSnapshot, UnknownReason};
use crate::list_max::{self, ListMaxBound, ListMaxRead};
use crate::schema::{
    FactDeclaration, GateUnknownHandling, IntegerCompareOp, LookupKey, MAX_CANDIDATES, RowFilter,
    SELECTION_POLICY_SCHEMA_VERSION, SelectionError, SelectionErrorCode, SelectionMode,
    SelectionPolicy, SortDirection, TermUnknownHandling, TieBreakKey, Transform, ValueRef,
    ValueType,
};
use crate::tables::{SelectionTables, TableReading};

/// The rows each `keyed_fact` reference reads, by its record list and its `where`, keyed by
/// the row's key column.
pub(crate) type ListIndex<'a> =
    BTreeMap<(&'a str, Option<&'a RowFilter>), BTreeMap<&'a str, &'a RecordRow>>;

/// One entry in the decision's reason chain: a registered code with its values (Workflow #378
/// A2-P), which a selection record persists, and developer text that is never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReason {
    pub outcome: Outcome,
    pub detail: String,
}

impl DecisionReason {
    /// A reason whose code carries no values.
    fn new(code: SelectionPolicyCode, detail: impl Into<String>) -> Self {
        Self::with(Outcome::new(code), detail)
    }

    /// A reason with its outcome built, values included.
    fn with(outcome: Outcome, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: detail.into(),
        }
    }

    /// A gate's or a scoring term's reason, naming the rule as `rule_id`.
    fn rule(code: SelectionPolicyCode, rule_id: &str, detail: impl Into<String>) -> Self {
        Self::with(Outcome::new(code).rule_id(rule_id), detail)
    }

    /// `selection_unknown`, with the unknown input's reason.
    fn unknown(reason: UnknownReason, detail: impl Into<String>) -> Self {
        Self::with(
            Outcome::new(SelectionPolicyCode::SelectionUnknown)
                .unknown_reason(UnknownReasonToken(reason)),
            detail,
        )
    }
}

/// An unknown input's reason as a token of the catalog vocabulary `unknown_reason`, so a
/// reason's outcome can carry it (Workflow #378 A2-P).
#[derive(Clone, Copy)]
struct UnknownReasonToken(UnknownReason);

impl VocabularyToken for UnknownReasonToken {
    const VOCABULARY: &'static str = "unknown_reason";

    fn token(self) -> &'static str {
        match self.0 {
            UnknownReason::FactMissing => "fact_missing",
            UnknownReason::FactExpired => "fact_expired",
            UnknownReason::FactStale => "fact_stale",
            UnknownReason::FactLowConfidence => "fact_low_confidence",
            UnknownReason::FactNotScalar => "fact_not_scalar",
            UnknownReason::FieldMissing => "field_missing",
            // The vocabulary token shares the registered code's spelling.
            UnknownReason::TypeMismatch => SharedCode::TypeMismatch.as_str(),
            UnknownReason::LookupMiss => "lookup_miss",
        }
    }
}

/// What one hard gate did to one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GateOutcome {
    Passed,
    Failed,
    UnknownSubstituted { reason: UnknownReason, passes: bool },
    UnknownDropped { reason: UnknownReason },
}

/// One gate's verdict on one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateResult {
    pub gate_id: String,
    pub outcome: GateOutcome,
}

/// What one scoring term did for one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TermOutcome {
    Scored {
        transformed_milli: i64,
    },
    UnknownSubstituted {
        reason: UnknownReason,
        transformed_milli: i64,
    },
    UnknownDropped {
        reason: UnknownReason,
    },
}

/// One scoring term's contribution to one candidate's score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TermResult {
    pub term_id: String,
    pub outcome: TermOutcome,
    pub weight_milli: i64,
    pub contribution_milli: i64,
}

/// Where one candidate ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// Passed every gate and carries a score and a rank.
    Ranked,
    /// A gate's predicate was decided against it.
    GateRejected,
    /// A gate or term met an unknown input the document answers by dropping the candidate.
    UnknownDropped,
}

/// One candidate's full breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateVerdict {
    pub candidate_id: String,
    pub status: CandidateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_milli: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    pub gates: Vec<GateResult>,
    pub terms: Vec<TermResult>,
    pub reasons: Vec<DecisionReason>,
    /// Workflow #357 DB-2 (M-357a v1.1 §3.4, D21): the candidate is dropped only by the table's
    /// own static data, which reading again cannot change. The select step does not count it as
    /// an unknown instance. Never written: no record carries it (register §1.65 TB8).
    #[serde(skip)]
    pub static_table_drop: bool,
}

/// What the decision as a whole came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionOutcome {
    /// The required candidates were chosen.
    Selected { count: u32 },
    /// Nothing was chosen and the document allows that.
    Empty,
    /// Fewer candidates survived than the document requires; nothing is chosen.
    Insufficient { surviving: u32, required: u32 },
    /// The declared tie-break order does not separate the candidates at the cut.
    Ambiguous { candidate_ids: Vec<String> },
    /// An unknown input reached a rule the document answers by ending the evaluation.
    Unknown {
        reason: UnknownReason,
        detail: String,
    },
}

impl SelectionOutcome {
    fn key<'a>(&self, keys: &'a crate::schema::OutcomeKeys) -> &'a str {
        match self {
            Self::Selected { .. } => &keys.selected,
            Self::Empty => &keys.empty,
            Self::Insufficient { .. } => &keys.insufficient,
            Self::Ambiguous { .. } => &keys.ambiguous,
            Self::Unknown { .. } => &keys.unknown,
        }
    }
}

/// One explained selection decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionDecision {
    pub schema_version: String,
    pub policy_id: String,
    pub policy_sha256: String,
    pub input_sha256: String,
    pub candidate_layout_id: String,
    pub fact_snapshot_id: String,
    pub evaluated_at_unix_ms: u64,
    pub outcome: SelectionOutcome,
    pub outcome_key: String,
    pub selected: Vec<String>,
    pub candidates: Vec<CandidateVerdict>,
    pub reasons: Vec<DecisionReason>,
}

#[derive(Serialize)]
struct EvaluationInput<'a> {
    candidates: &'a [Candidate],
    facts: &'a SelectionFactSnapshot,
    now_unix_ms: u64,
    /// Workflow #357 DB-2 (M-357a v1.1 §3.7): the table rows the evaluation reads, only when the
    /// document declares a table, so a document without one keeps its `input_sha256`.
    #[serde(skip_serializing_if = "Option::is_none")]
    tables: Option<BTreeMap<String, Vec<BTreeMap<String, serde_json::Value>>>>,
}

/// Evaluates one policy over one candidate set and one fact snapshot.
///
/// The returned decision holds every candidate's breakdown, the chosen identifiers, and the
/// reason chain. An `Err` means the inputs could not be evaluated at all: an invalid
/// document, a malformed candidate set, or integer overflow. An unknown input is not an
/// error; it produces a typed unknown outcome or a dropped candidate, as the document says.
/// A document that declares a table needs [`evaluate_with_tables`]: here it is
/// `dangling_reference`.
pub fn evaluate(
    policy: &SelectionPolicy,
    candidates: &[Candidate],
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<SelectionDecision, SelectionError> {
    evaluate_with_tables(
        policy,
        SelectionTables::none(),
        candidates,
        facts,
        now_unix_ms,
    )
}

/// [`evaluate`] with the shared tables the document declares (Workflow #357 DB-2, D15): the one
/// scorer. A declared table `tables` does not hold is `dangling_reference`. The tables are read
/// here from memory; no file is read.
pub fn evaluate_with_tables(
    policy: &SelectionPolicy,
    tables: &SelectionTables,
    candidates: &[Candidate],
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<SelectionDecision, SelectionError> {
    policy.validate()?;
    check_candidate_set(candidates)?;
    tables.check_bound(policy)?;

    let policy_sha256 = canonical_sha256(policy)?;
    let read_rows = policy.uses_tables().then(|| {
        let resolver = Resolver::new(policy, facts, ListIndex::new(), tables, now_unix_ms);
        tables.read_rows(
            policy,
            candidates,
            &|key_field, candidate| match resolver.read_field(key_field, candidate) {
                Ok(ScalarValue::String(key)) => Some(key),
                _ => None,
            },
            &|fact_key| resolver.read_fact(fact_key),
            &|lists| list_max::union_keys(policy, lists, facts, now_unix_ms).ok(),
        )
    });
    let input_sha256 = canonical_sha256(&EvaluationInput {
        candidates,
        facts,
        now_unix_ms,
        tables: read_rows,
    })?;
    let mut reasons = vec![
        DecisionReason::new(SelectionPolicyCode::SelectionPolicyIdentity, &policy_sha256),
        DecisionReason::new(SelectionPolicyCode::SelectionInputsIdentity, &input_sha256),
        DecisionReason::new(
            SelectionPolicyCode::SelectionRequirementDeclared,
            format!(
                "{:?} required_count={}",
                policy.selection.mode, policy.selection.required_count
            ),
        ),
    ];

    // Every declared list is read before any candidate, so `empty` is reachable only after the
    // lists were read, even with no candidate at all.
    let lists = match read_lists(policy, facts, now_unix_ms) {
        Ok(lists) => lists,
        Err(ListUnreadable { reason, detail }) => {
            reasons.push(DecisionReason::unknown(reason, &detail));
            return Ok(finish(
                policy,
                policy_sha256,
                input_sha256,
                facts,
                now_unix_ms,
                SelectionOutcome::Unknown { reason, detail },
                Vec::new(),
                Vec::new(),
                reasons,
            ));
        }
    };
    // Workflow #428 S168-5a: each distinct list maximum is read once, after the lists and before
    // any candidate (also with none), and says what it read where it is read.
    let list_maxima = list_max::read_all(policy, tables, facts, now_unix_ms);
    for read in &list_maxima {
        list_max_reasons(read, &mut reasons);
    }
    let mut resolver = Resolver::new(policy, facts, lists, tables, now_unix_ms);
    resolver.list_maxima = list_maxima
        .into_iter()
        .map(|read| ListMaxValue {
            reference: read.reference,
            at_least: read.reading.at_least(),
            value: read.reading.value,
        })
        .collect();

    let mut verdicts = Vec::with_capacity(candidates.len());
    let mut survivors: Vec<Ranked> = Vec::new();
    // Workflow #428 S168-LB1b: the gates and terms a lower bound settled for some candidate.
    let mut on_bound = BoundSettled::default();
    for candidate in candidates {
        match assess(policy, candidate, &resolver, &mut on_bound)? {
            Assessment::Aborted { verdict, reason } => {
                verdicts.push(verdict);
                reasons.push(DecisionReason::unknown(reason.0, &reason.1));
                let outcome = SelectionOutcome::Unknown {
                    reason: reason.0,
                    detail: reason.1,
                };
                return Ok(finish(
                    policy,
                    policy_sha256,
                    input_sha256,
                    facts,
                    now_unix_ms,
                    outcome,
                    Vec::new(),
                    verdicts,
                    reasons,
                ));
            }
            Assessment::Settled { verdict, ranked } => {
                if let Some(ranked) = ranked {
                    survivors.push(ranked);
                }
                verdicts.push(verdict);
            }
        }
    }

    survivors.sort_by(compare);
    for (index, ranked) in survivors.iter().enumerate() {
        let rank = index as u32 + 1;
        let verdict = verdicts
            .iter_mut()
            .find(|verdict| verdict.candidate_id == ranked.candidate_id);
        if let Some(verdict) = verdict {
            verdict.rank = Some(rank);
            verdict.reasons.push(DecisionReason::new(
                SelectionPolicyCode::CandidateRanked,
                format!("rank={rank}"),
            ));
        }
    }

    let requested = match policy.selection.mode {
        SelectionMode::ExactlyOne => 1usize,
        SelectionMode::TopK => policy.selection.required_count as usize,
        SelectionMode::NoneAllowed => {
            (policy.selection.required_count as usize).min(survivors.len())
        }
    };
    let outcome = if requested > survivors.len() {
        SelectionOutcome::Insufficient {
            surviving: survivors.len() as u32,
            required: requested as u32,
        }
    } else if requested == 0 {
        SelectionOutcome::Empty
    } else if requested < survivors.len() && tied(&survivors[requested - 1], &survivors[requested])
    {
        SelectionOutcome::Ambiguous {
            candidate_ids: vec![
                survivors[requested - 1].candidate_id.clone(),
                survivors[requested].candidate_id.clone(),
            ],
        }
    } else {
        SelectionOutcome::Selected {
            count: requested as u32,
        }
    };
    let selected = match &outcome {
        SelectionOutcome::Selected { count } => survivors
            .iter()
            .take(*count as usize)
            .map(|ranked| ranked.candidate_id.clone())
            .collect(),
        _ => Vec::new(),
    };
    // Workflow #428 S168-LB1b: a decision made on a lower bound says so once, after the
    // candidates' assessment and before the outcome's reasons. An evaluation that ended unknown
    // returned above and carries none.
    if let Some(reason) = on_bound.reason(policy) {
        reasons.push(reason);
    }
    match &outcome {
        SelectionOutcome::Selected { count } => reasons.push(DecisionReason::new(
            SelectionPolicyCode::SelectionSelected,
            format!("{count} of {} surviving", survivors.len()),
        )),
        SelectionOutcome::Empty => reasons.push(DecisionReason::new(
            SelectionPolicyCode::SelectionEmpty,
            format!("{} surviving, none required", survivors.len()),
        )),
        SelectionOutcome::Insufficient {
            surviving,
            required,
        } => reasons.push(DecisionReason::new(
            SelectionPolicyCode::SelectionInsufficient,
            format!("{surviving} surviving below required {required}"),
        )),
        SelectionOutcome::Ambiguous { candidate_ids } => reasons.push(DecisionReason::new(
            SelectionPolicyCode::SelectionAmbiguous,
            format!("tie-break leaves {} at the cut", candidate_ids.join(", ")),
        )),
        SelectionOutcome::Unknown { .. } => {}
    }
    for verdict in &mut verdicts {
        if selected.contains(&verdict.candidate_id) {
            verdict.reasons.push(DecisionReason::new(
                SelectionPolicyCode::CandidateSelected,
                "chosen",
            ));
        }
    }

    Ok(finish(
        policy,
        policy_sha256,
        input_sha256,
        facts,
        now_unix_ms,
        outcome,
        selected,
        verdicts,
        reasons,
    ))
}

/// The reasons one list maximum's read adds to the decision's chain (Workflow #428 S168-5a,
/// "Audit"): a `term_fallback_used` per pin that used its `otherwise`, naming the pin's column
/// and the gate or term that reads the reference first, then `selection_table_list_max` with
/// the table, its column, the lists in the order of `lists`, the number of distinct keys of the
/// union and either the largest value with the smallest key that holds it, or nothing for
/// `empty`, or the unknown reason. Workflow #428 S168-LB1b: a known read of a reference that
/// names a `lower_bound_column` adds `bound` and `unknown_count`, and, at least, `lower_bound`
/// and `lower_bound_identity` in the place of `maximum` and `maximum_identity`.
fn list_max_reasons(read: &ListMaxRead<'_>, reasons: &mut Vec<DecisionReason>) {
    let ValueRef::TableMaxOverList {
        table_id,
        value_column,
        lists,
        ..
    } = read.reference
    else {
        return;
    };
    for column in &read.reading.otherwise_columns {
        reasons.push(DecisionReason::with(
            Outcome::new(SelectionPolicyCode::TermFallbackUsed)
                .rule_id(read.rule_id)
                .table_column(column.as_str()),
            format!("list maximum of table `{table_id}` read pin `{column}` through its otherwise"),
        ));
    }
    let reading = &read.reading;
    let outcome = Outcome::new(SelectionPolicyCode::SelectionTableListMax)
        .table_id(table_id.as_str())
        .table_column(value_column.as_str())
        .fact_keys(lists.iter().map(|list| list.fact_key.clone()))
        .count(reading.count);
    // Workflow #428 S168-LB1b: a known read of a reference that names a `lower_bound_column`
    // says whether it is exact and how many keys have no value; at least, it names L and its key
    // in the place of the maximum's.
    let outcome = match reading.bound {
        Some(bound) => outcome.bound(bound).unknown_count(reading.unknown_count),
        None => outcome,
    };
    let (outcome, detail) = match (&reading.value, &reading.maximum_identity) {
        (Ok(ScalarValue::Integer(lower_bound)), Some(identity))
            if reading.bound == Some(ListMaxBound::AtLeast) =>
        {
            (
                outcome
                    .lower_bound(*lower_bound)
                    .lower_bound_identity(identity.as_str()),
                format!(
                    "table `{table_id}` at least {lower_bound} identity={identity} with {} keys \
                     without a value",
                    reading.unknown_count
                ),
            )
        }
        (Ok(ScalarValue::Integer(maximum)), Some(identity)) => (
            outcome
                .maximum(*maximum)
                .maximum_identity(identity.as_str()),
            format!("table `{table_id}` maximum={maximum} identity={identity}"),
        ),
        (Ok(_), _) => (
            outcome,
            format!("table `{table_id}` no listed key of the class: empty"),
        ),
        (Err(reason), _) => (
            outcome.unknown_reason(UnknownReasonToken(*reason)),
            format!("table `{table_id}` list maximum unknown: {reason:?}"),
        ),
    };
    reasons.push(DecisionReason::with(outcome, detail));
}

/// One input a gate left undecided, or a term could not use, under [`evaluate_collect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownInput {
    /// The gate or term that met the input.
    pub rule_id: String,
    /// What it read: a fact key (`fact` source) or a candidate field name (`field` source).
    pub input: ValueRef,
    pub reason: UnknownReason,
}

/// One input a gate or term read as a known value, under [`evaluate_collect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownInput {
    pub input: ValueRef,
    /// For a `fact` source, the last instant the read stays known: the record's own expiry or
    /// its publication plus the document's `max_age_ms`, whichever comes first. `None` for a
    /// candidate field, whose freshness only the caller knows.
    pub known_until_unix_ms: Option<u64>,
}

/// Where one candidate ended up under [`evaluate_collect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectedVerdict {
    /// Every gate held and every term read a known value.
    Scored { score_milli: i64 },
    /// A gate's predicate was decided against the candidate.
    GateRejected { gate_id: String },
    /// A gate was left undecided or a term could not use its input. Every such input is
    /// listed once, gates before terms, in document order.
    Unknown { inputs: Vec<UnknownInput> },
}

/// One candidate's result under [`evaluate_collect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedCandidate {
    pub candidate_id: String,
    pub verdict: CollectedVerdict,
    /// Every input the candidate's gates and terms read as a known value, each once, in the
    /// order they were read.
    pub known_inputs: Vec<KnownInput>,
}

/// Evaluates every candidate under the collect option: the call a scheduling layer makes.
///
/// A gate or term that meets an unknown input neither ends the evaluation, nor drops the
/// candidate, nor substitutes a value: the `on_unknown` clauses the document declares have no
/// effect here. The candidate's verdict lists every unknown input instead, so the caller can
/// treat each one. Evaluation goes on past an undecided gate so that every unknown input of
/// the candidate is listed; a gate decided against it still rejects it, because the gates are
/// a conjunction. Tie-break keys are not read. The document and the hashes are not involved:
/// nothing is selected, and the result is one entry per candidate, in input order.
///
/// An `Err` means the same as for [`evaluate`]: the inputs could not be evaluated at all. A
/// document that declares a record list is refused with `type_mismatch`: only a task's select
/// step reads one, through [`evaluate`] and its up-front list check, and the scheduling consumer
/// refuses such a document at catalog compile for the same reason. A document that declares a
/// shared table is refused the same way (Workflow #357 DB-2, register §1.65 TB10): dispatch value
/// goes through the resource line, so scheduling reads no table. So is a document that reads a
/// list maximum (Workflow #428 S168-5a), which reads both.
pub fn evaluate_collect(
    policy: &SelectionPolicy,
    candidates: &[Candidate],
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<Vec<CollectedCandidate>, SelectionError> {
    policy.validate()?;
    if policy.uses_list_maxima() {
        return Err(SelectionError::new(
            SelectionErrorCode::TypeMismatch,
            "value_source=table_max_over_list option=collect",
        ));
    }
    if policy.uses_record_lists() {
        return Err(SelectionError::new(
            SelectionErrorCode::TypeMismatch,
            "value_type=record_list option=collect",
        ));
    }
    if policy.uses_tables() {
        return Err(SelectionError::new(
            SelectionErrorCode::TypeMismatch,
            "tables=declared option=collect",
        ));
    }
    check_candidate_set(candidates)?;
    let resolver = Resolver::new(
        policy,
        facts,
        ListIndex::new(),
        SelectionTables::none(),
        now_unix_ms,
    );
    candidates
        .iter()
        .map(|candidate| collect_candidate(policy, candidate, &resolver))
        .collect()
}

fn check_candidate_set(candidates: &[Candidate]) -> Result<(), SelectionError> {
    if candidates.len() > MAX_CANDIDATES {
        return Err(SelectionError::new(
            SelectionErrorCode::LimitExceeded,
            format!(
                "{} candidates exceed the {MAX_CANDIDATES} limit",
                candidates.len()
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        if candidate.candidate_id.is_empty() {
            return Err(SelectionError::new(
                SelectionErrorCode::MissingRequiredField,
                "a candidate carries an empty candidate_id".to_owned(),
            ));
        }
        if !seen.insert(candidate.candidate_id.as_str()) {
            return Err(SelectionError::new(
                SelectionErrorCode::DuplicateId,
                format!("candidate `{}` appears twice", candidate.candidate_id),
            ));
        }
    }
    Ok(())
}

fn collect_candidate(
    policy: &SelectionPolicy,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
) -> Result<CollectedCandidate, SelectionError> {
    let mut reads = Reads::recording();
    let mut unknown = Vec::new();

    for gate in &policy.gates {
        let start = reads.unknown.len();
        match truth(&gate.predicate, candidate, resolver, &mut reads) {
            Truth::True => {}
            Truth::False => {
                return Ok(CollectedCandidate {
                    candidate_id: candidate.candidate_id.clone(),
                    verdict: CollectedVerdict::GateRejected {
                        gate_id: gate.gate_id.clone(),
                    },
                    known_inputs: reads.known,
                });
            }
            Truth::Unknown(_) => {
                for (input, reason) in reads.unknown.drain(start..) {
                    note_unknown(&mut unknown, &gate.gate_id, input, reason);
                }
            }
        }
        reads.unknown.truncate(start);
    }

    // The score only matters while every input is known; once one is not, the terms are read
    // only to list their own unknown inputs.
    let mut total = unknown.is_empty().then_some(0_i128);
    for term in &policy.scoring {
        let resolved = resolver
            .read(&term.value, candidate)
            .and_then(|value| transform(&term.transform, &value));
        let transformed_milli = match resolved {
            Ok(transformed_milli) => {
                reads.record_known(resolver, &term.value);
                transformed_milli
            }
            Err(reason) => {
                note_unknown(&mut unknown, &term.term_id, term.value.clone(), reason);
                total = None;
                continue;
            }
        };
        if let Some(sum) = total.as_mut() {
            let product = i128::from(transformed_milli) * i128::from(term.weight_milli) / 1_000;
            let contribution_milli = i64::try_from(product).map_err(|_| {
                SelectionError::new(
                    SelectionErrorCode::ArithmeticOverflow,
                    format!("term `{}` overflows a 64-bit milli score", term.term_id),
                )
            })?;
            *sum += i128::from(contribution_milli);
        }
    }

    let verdict = if unknown.is_empty() {
        let score_milli = i64::try_from(total.unwrap_or_default()).map_err(|_| {
            SelectionError::new(
                SelectionErrorCode::ArithmeticOverflow,
                format!(
                    "candidate `{}` overflows a 64-bit milli score",
                    candidate.candidate_id
                ),
            )
        })?;
        CollectedVerdict::Scored { score_milli }
    } else {
        CollectedVerdict::Unknown { inputs: unknown }
    };
    Ok(CollectedCandidate {
        candidate_id: candidate.candidate_id.clone(),
        verdict,
        known_inputs: reads.known,
    })
}

fn note_unknown(
    unknown: &mut Vec<UnknownInput>,
    rule_id: &str,
    input: ValueRef,
    reason: UnknownReason,
) {
    let entry = UnknownInput {
        rule_id: rule_id.to_owned(),
        input,
        reason,
    };
    if !unknown.contains(&entry) {
        unknown.push(entry);
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    policy: &SelectionPolicy,
    policy_sha256: String,
    input_sha256: String,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
    outcome: SelectionOutcome,
    selected: Vec<String>,
    candidates: Vec<CandidateVerdict>,
    mut reasons: Vec<DecisionReason>,
) -> SelectionDecision {
    let outcome_key = outcome.key(&policy.applies_to.outcome_keys).to_owned();
    reasons.push(DecisionReason::new(
        SelectionPolicyCode::SelectionOutcomeKeyChosen,
        &outcome_key,
    ));
    SelectionDecision {
        schema_version: SELECTION_POLICY_SCHEMA_VERSION.to_owned(),
        policy_id: policy.policy_id.clone(),
        policy_sha256,
        input_sha256,
        candidate_layout_id: policy.applies_to.candidate_layout_id.clone(),
        fact_snapshot_id: facts.snapshot_id.clone(),
        evaluated_at_unix_ms: now_unix_ms,
        outcome,
        outcome_key,
        selected,
        candidates,
        reasons,
    }
}

/// Why the up-front list check failed: the typed reason of the `unknown` decision, and its
/// detail, the list's fact key (and the key two rows share).
pub(crate) struct ListUnreadable {
    pub(crate) reason: UnknownReason,
    pub(crate) detail: String,
}

/// The up-front list check. Every record list the document declares is read whole, in
/// document order; then the rows each `keyed_fact` reference, and each list a list maximum reads
/// (Workflow #428 S168-5a), selects with its `where` are indexed by key. An unreadable list, or
/// one key held by two selected rows, is the reason of an `unknown` decision, with the list's
/// fact key as its detail.
pub(crate) fn read_lists<'a>(
    policy: &'a SelectionPolicy,
    facts: &'a SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<ListIndex<'a>, ListUnreadable> {
    let mut lists = BTreeMap::new();
    for declaration in &policy.facts {
        if let ValueType::RecordList { key_column, .. } = &declaration.value_type {
            let rows = facts
                .resolve_list(declaration, now_unix_ms)
                .map_err(|reason| ListUnreadable {
                    reason,
                    detail: format!("fact_key={}", declaration.fact_key),
                })?;
            lists.insert(declaration.fact_key.as_str(), (key_column.as_str(), rows));
        }
    }
    // Workflow #428 S168-5a: each list a list maximum reads is a reference with its `where` too.
    let mut references = Vec::new();
    for value in policy.value_refs() {
        match value {
            ValueRef::KeyedFact {
                fact_key,
                row_filter,
                ..
            } => references.push((fact_key, row_filter)),
            ValueRef::TableMaxOverList { lists: read, .. } => {
                references.extend(read.iter().map(|list| (&list.fact_key, &list.row_filter)))
            }
            _ => {}
        }
    }
    let mut index = ListIndex::new();
    for (fact_key, row_filter) in references {
        let reference = (fact_key.as_str(), row_filter.as_ref());
        if index.contains_key(&reference) {
            continue;
        }
        let malformed = || ListUnreadable {
            reason: UnknownReason::TypeMismatch,
            detail: format!("fact_key={fact_key}"),
        };
        let &(key_column, rows) = lists.get(fact_key.as_str()).ok_or_else(malformed)?;
        let mut keyed = BTreeMap::new();
        for row in rows {
            if let Some(filter) = row_filter
                && !matches!(row.get(&filter.column), Some(ScalarValue::String(value)) if *value == filter.equals)
            {
                continue;
            }
            let Some(ScalarValue::String(key)) = row.get(key_column) else {
                return Err(malformed());
            };
            if keyed.insert(key.as_str(), row).is_some() {
                return Err(ListUnreadable {
                    reason: UnknownReason::TypeMismatch,
                    detail: format!("fact_key={fact_key} duplicated_key={key}"),
                });
            }
        }
        index.insert(reference, keyed);
    }
    Ok(index)
}

struct Resolver<'a> {
    fields: BTreeMap<&'a str, &'a ValueType>,
    facts: BTreeMap<&'a str, &'a FactDeclaration>,
    snapshot: &'a SelectionFactSnapshot,
    lists: ListIndex<'a>,
    tables: &'a SelectionTables,
    now_unix_ms: u64,
    /// Workflow #428 S168-5a: what each distinct list maximum read, before any candidate.
    list_maxima: Vec<ListMaxValue<'a>>,
}

/// One distinct list maximum as the candidates read it (Workflow #428 S168-5a).
struct ListMaxValue<'a> {
    reference: &'a ValueRef,
    /// The value, `empty`, L for a read that is at least, or why the read is unknown.
    value: Result<ScalarValue, UnknownReason>,
    /// Workflow #428 S168-LB1b: the read is "at least" its value, the range [L, +∞).
    at_least: bool,
}

impl<'a> Resolver<'a> {
    fn new(
        policy: &'a SelectionPolicy,
        snapshot: &'a SelectionFactSnapshot,
        lists: ListIndex<'a>,
        tables: &'a SelectionTables,
        now_unix_ms: u64,
    ) -> Self {
        Self {
            fields: policy
                .fields
                .iter()
                .map(|declaration| (declaration.name.as_str(), &declaration.value_type))
                .collect(),
            facts: policy
                .facts
                .iter()
                .map(|declaration| (declaration.fact_key.as_str(), declaration))
                .collect(),
            snapshot,
            lists,
            tables,
            now_unix_ms,
            list_maxima: Vec::new(),
        }
    }

    /// The value one list maximum read for the whole evaluation (Workflow #428 S168-5a). A
    /// reference read no maximum for (none was read before the candidates) is no known value.
    fn list_max(&self, reference: &ValueRef) -> Result<ScalarValue, UnknownReason> {
        self.list_maxima
            .iter()
            .find(|read| read.reference == reference)
            .map_or(Err(UnknownReason::TypeMismatch), |read| read.value.clone())
    }

    /// Whether `reference` is a list maximum read as "at least" its value (Workflow #428
    /// S168-LB1b): [`Self::read`] gives L, and every reader takes the range [L, +∞).
    fn reads_at_least(&self, reference: &ValueRef) -> bool {
        reference.is_list_max()
            && self
                .list_maxima
                .iter()
                .any(|read| read.reference == reference && read.at_least)
    }

    /// A declared scalar fact in the pinned snapshot, or why it is unknown.
    fn read_fact(&self, fact_key: &str) -> Result<ScalarValue, UnknownReason> {
        let declared = self.facts.get(fact_key).ok_or(UnknownReason::FactMissing)?;
        self.snapshot.resolve(declared, self.now_unix_ms).cloned()
    }

    /// One `table` read for `candidate` (§3.4), its key being the candidate's own; an `Err` is a
    /// key the candidate does not carry, which is no fact about the table.
    fn table_reading(
        &self,
        reference: &ValueRef,
        candidate: &Candidate,
    ) -> Result<TableReading, UnknownReason> {
        let ValueRef::Table { key_field, .. } = reference else {
            return Err(UnknownReason::TypeMismatch);
        };
        let ScalarValue::String(key) = self.read_field(key_field, candidate)? else {
            return Err(UnknownReason::TypeMismatch);
        };
        Ok(self
            .tables
            .read(reference, &key, &|fact_key| self.read_fact(fact_key)))
    }

    /// [`Self::read`], noting in `reads` what a gate or a term must report of a `table` read.
    fn read_noting(
        &self,
        reference: &ValueRef,
        candidate: &Candidate,
        reads: &mut Reads,
    ) -> Result<ScalarValue, UnknownReason> {
        if !reference.is_table() {
            let read = self.read(reference, candidate);
            if read.is_err() {
                reads.marks.other_unknown = true;
            }
            return read;
        }
        match self.table_reading(reference, candidate) {
            Ok(reading) => reads.marks.note(reading),
            Err(reason) => {
                reads.marks.other_unknown = true;
                Err(reason)
            }
        }
    }

    /// The last instant a `fact` read stays known: the record's own expiry or its publication
    /// plus the document's `max_age_ms`, whichever comes first. A field has none here.
    fn known_until(&self, reference: &ValueRef) -> Option<u64> {
        let ValueRef::Fact { fact_key } = reference else {
            return None;
        };
        let declared = self.facts.get(fact_key.as_str())?;
        match self.snapshot.facts.get(fact_key)? {
            crate::facts::SelectionFactEntry::Published {
                published_at_unix_ms,
                expires_at_unix_ms,
                ..
            } => {
                let fresh_until = published_at_unix_ms.saturating_add(declared.max_age_ms);
                Some(expires_at_unix_ms.map_or(fresh_until, |expiry| expiry.min(fresh_until)))
            }
            // A record list is never a known `fact` read: it resolves to `fact_not_scalar`.
            crate::facts::SelectionFactEntry::Unusable { .. }
            | crate::facts::SelectionFactEntry::RecordList { .. } => None,
        }
    }

    fn read(
        &self,
        reference: &ValueRef,
        candidate: &Candidate,
    ) -> Result<ScalarValue, UnknownReason> {
        match reference {
            ValueRef::Field { field } => self.read_field(field, candidate),
            ValueRef::Fact { fact_key } => self.read_fact(fact_key),
            ValueRef::Table { .. } => self.table_reading(reference, candidate)?.value,
            ValueRef::TableMaxOverList { .. } => self.list_max(reference),
            ValueRef::KeyedFact {
                fact_key,
                key_field,
                value_column,
                row_filter,
                absent,
            } => {
                let ScalarValue::String(key) = self.read_field(key_field, candidate)? else {
                    return Err(UnknownReason::TypeMismatch);
                };
                let rows = self
                    .lists
                    .get(&(fact_key.as_str(), row_filter.as_ref()))
                    .ok_or(UnknownReason::FactMissing)?;
                // No row is a known value: the list was read, and this key is not on it.
                match rows.get(key.as_str()) {
                    Some(row) => row
                        .get(value_column)
                        .cloned()
                        .ok_or(UnknownReason::TypeMismatch),
                    None => Ok(absent.clone()),
                }
            }
        }
    }

    fn read_field(&self, field: &str, candidate: &Candidate) -> Result<ScalarValue, UnknownReason> {
        let declared = self.fields.get(field).ok_or(UnknownReason::FieldMissing)?;
        let value = candidate
            .fields
            .get(field)
            .ok_or(UnknownReason::FieldMissing)?;
        if value.matches(declared) {
            Ok(value.clone())
        } else {
            Err(UnknownReason::TypeMismatch)
        }
    }
}

struct Ranked {
    candidate_id: String,
    score_milli: i64,
    keys: Vec<(Result<ScalarValue, UnknownReason>, SortDirection)>,
}

enum Assessment {
    Aborted {
        verdict: CandidateVerdict,
        reason: (UnknownReason, String),
    },
    Settled {
        verdict: CandidateVerdict,
        ranked: Option<Ranked>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    True,
    False,
    Unknown(UnknownReason),
}

/// The inputs one candidate's predicates read, kept only for [`evaluate_collect`].
///
/// `unknown` holds the unknown inputs of the predicate being decided. A conjunction or a
/// disjunction that its known members decide drops the unknown inputs its members recorded,
/// because they did not matter, so what is left when a gate stays undecided is exactly the
/// set of inputs that left it undecided.
struct Reads {
    recording: bool,
    known: Vec<KnownInput>,
    unknown: Vec<(ValueRef, UnknownReason)>,
    /// Workflow #357 DB-2: what the `table` reads of the gate or term being decided came to.
    marks: TableMarks,
}

/// What a gate's or a term's reads noted (M-357a v1.1 §5): the two marks a `table` read carries
/// to [`assess`], and whether anything else was unknown. Neither mark reaches a record except as
/// the reason `term_fallback_used`.
#[derive(Default)]
struct TableMarks {
    used_fallback: bool,
    otherwise_column: Option<String>,
    static_unknown: bool,
    other_unknown: bool,
    /// Workflow #428 S168-LB1b: a comparison with a list maximum read as a lower bound was
    /// settled, true or false, by that bound.
    settled_on_bound: bool,
}

impl TableMarks {
    fn note(&mut self, reading: TableReading) -> Result<ScalarValue, UnknownReason> {
        self.used_fallback |= reading.used_fallback;
        if self.otherwise_column.is_none() {
            self.otherwise_column = reading.otherwise_column;
        }
        if reading.value.is_err() {
            if reading.static_unknown {
                self.static_unknown = true;
            } else {
                self.other_unknown = true;
            }
        }
        reading.value
    }

    /// Whether every Unknown the gate or term met is the table's own static data (D21).
    fn only_static(&self) -> bool {
        self.static_unknown && !self.other_unknown
    }

    /// `term_fallback_used` for the rule, at most one per candidate and rule; a pin's `otherwise`
    /// names its column.
    fn fallback_reason(&self, rule_id: &str) -> Option<DecisionReason> {
        if !self.used_fallback && self.otherwise_column.is_none() {
            return None;
        }
        let outcome = Outcome::new(SelectionPolicyCode::TermFallbackUsed).rule_id(rule_id);
        let outcome = match &self.otherwise_column {
            Some(column) => outcome.table_column(column.as_str()),
            None => outcome,
        };
        Some(DecisionReason::with(
            outcome,
            format!("rule `{rule_id}` read a declared fallback"),
        ))
    }
}

impl Reads {
    fn recording() -> Self {
        Self {
            recording: true,
            known: Vec::new(),
            unknown: Vec::new(),
            marks: TableMarks::default(),
        }
    }

    fn ignored() -> Self {
        Self {
            recording: false,
            known: Vec::new(),
            unknown: Vec::new(),
            marks: TableMarks::default(),
        }
    }

    fn record_known(&mut self, resolver: &Resolver<'_>, input: &ValueRef) {
        if self.recording && !self.known.iter().any(|known| known.input == *input) {
            self.known.push(KnownInput {
                input: input.clone(),
                known_until_unix_ms: resolver.known_until(input),
            });
        }
    }

    fn record_unknown(&mut self, input: &ValueRef, reason: UnknownReason) {
        if self.recording {
            self.unknown.push((input.clone(), reason));
        }
    }
}

/// Workflow #428 S168-LB1b (contracts/selection-policy.md, "What a person sees"): the gates and
/// the terms in which a list maximum read as a lower bound settled something for a candidate (a
/// comparison the bound settled in a gate whose verdict is known, a threshold the bound settled,
/// an identity term taken at L), each once, by its place: the gates in document order, then the
/// terms, the order of "One read".
#[derive(Default)]
struct BoundSettled {
    gates: BTreeSet<usize>,
    terms: BTreeSet<usize>,
}

impl BoundSettled {
    /// `selection_decided_on_lower_bound` with the first such rule and how many there were, or
    /// none for a decision no lower bound settled anything of.
    fn reason(&self, policy: &SelectionPolicy) -> Option<DecisionReason> {
        let first = match (self.gates.first(), self.terms.first()) {
            (Some(&gate), _) => policy.gates.get(gate).map(|gate| gate.gate_id.as_str()),
            (None, Some(&term)) => policy.scoring.get(term).map(|term| term.term_id.as_str()),
            (None, None) => None,
        }?;
        let count = self.gates.len() + self.terms.len();
        Some(DecisionReason::with(
            Outcome::new(SelectionPolicyCode::SelectionDecidedOnLowerBound)
                .rule_id(first)
                .count(count),
            format!("{count} gates and terms decided on a lower bound, the first `{first}`"),
        ))
    }
}

fn assess(
    policy: &SelectionPolicy,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
    on_bound: &mut BoundSettled,
) -> Result<Assessment, SelectionError> {
    let mut verdict = CandidateVerdict {
        candidate_id: candidate.candidate_id.clone(),
        status: CandidateStatus::Ranked,
        score_milli: None,
        rank: None,
        gates: Vec::new(),
        terms: Vec::new(),
        reasons: Vec::new(),
        static_table_drop: false,
    };

    let mut unrecorded = Reads::ignored();
    for (gate_index, gate) in policy.gates.iter().enumerate() {
        unrecorded.marks = TableMarks::default();
        let decided = truth(&gate.predicate, candidate, resolver, &mut unrecorded);
        // Workflow #428 S168-LB1b: a known verdict to which a lower bound settled a comparison.
        if unrecorded.marks.settled_on_bound && !matches!(decided, Truth::Unknown(_)) {
            on_bound.gates.insert(gate_index);
        }
        let (outcome, keep) = match decided {
            Truth::True => (GateOutcome::Passed, true),
            Truth::False => (GateOutcome::Failed, false),
            Truth::Unknown(reason) => match gate.on_unknown {
                GateUnknownHandling::DropCandidate => {
                    (GateOutcome::UnknownDropped { reason }, false)
                }
                GateUnknownHandling::SubstituteVerdict { passes } => {
                    (GateOutcome::UnknownSubstituted { reason, passes }, passes)
                }
                GateUnknownHandling::AbortEvaluation => {
                    verdict.gates.push(GateResult {
                        gate_id: gate.gate_id.clone(),
                        outcome: GateOutcome::UnknownDropped { reason },
                    });
                    verdict.status = CandidateStatus::UnknownDropped;
                    let detail = format!(
                        "candidate `{}` gate `{}`: {reason:?}",
                        candidate.candidate_id, gate.gate_id
                    );
                    verdict.reasons.push(DecisionReason::rule(
                        SelectionPolicyCode::GateUnknownAborted,
                        &gate.gate_id,
                        &detail,
                    ));
                    return Ok(Assessment::Aborted {
                        verdict,
                        reason: (reason, detail),
                    });
                }
            },
        };
        let substituted = matches!(outcome, GateOutcome::UnknownSubstituted { .. });
        let dropped = matches!(outcome, GateOutcome::UnknownDropped { .. });
        verdict.gates.push(GateResult {
            gate_id: gate.gate_id.clone(),
            outcome,
        });
        if substituted {
            verdict.reasons.push(DecisionReason::rule(
                SelectionPolicyCode::GateUnknownSubstituted,
                &gate.gate_id,
                format!("gate `{}` used the declared verdict", gate.gate_id),
            ));
        }
        if let Some(reason) = unrecorded.marks.fallback_reason(&gate.gate_id) {
            verdict.reasons.push(reason);
        }
        if !keep {
            verdict.status = if dropped {
                verdict.reasons.push(DecisionReason::rule(
                    SelectionPolicyCode::GateUnknownDropped,
                    &gate.gate_id,
                    format!("gate `{}` met an unknown input", gate.gate_id),
                ));
                verdict.static_table_drop = unrecorded.marks.only_static();
                CandidateStatus::UnknownDropped
            } else {
                verdict.reasons.push(DecisionReason::rule(
                    SelectionPolicyCode::GateRejected,
                    &gate.gate_id,
                    format!("gate `{}` does not hold", gate.gate_id),
                ));
                CandidateStatus::GateRejected
            };
            return Ok(Assessment::Settled {
                verdict,
                ranked: None,
            });
        }
    }

    let mut total: i128 = 0;
    for (term_index, term) in policy.scoring.iter().enumerate() {
        let mut term_reads = Reads::ignored();
        let read = resolver.read_noting(&term.value, candidate, &mut term_reads);
        // A value the term's own transform cannot use is the document's, not the table's.
        let static_unknown = read.is_err() && term_reads.marks.only_static();
        // Workflow #428 S168-LB1b: a list maximum read as a lower bound is the range [L, +∞):
        // `identity` takes L, `threshold` is settled only once L reaches `at_least`.
        let at_least = resolver.reads_at_least(&term.value);
        let resolved = read.and_then(|value| {
            if at_least {
                transform_at_least(&term.transform, &value)
            } else {
                transform(&term.transform, &value)
            }
        });
        if at_least && resolved.is_ok() {
            on_bound.terms.insert(term_index);
        }
        let (outcome, keep) = match resolved {
            Ok(transformed_milli) => (TermOutcome::Scored { transformed_milli }, true),
            Err(reason) => match term.on_unknown {
                TermUnknownHandling::DropCandidate => {
                    (TermOutcome::UnknownDropped { reason }, false)
                }
                TermUnknownHandling::SubstituteMilli { value_milli } => (
                    TermOutcome::UnknownSubstituted {
                        reason,
                        transformed_milli: value_milli,
                    },
                    true,
                ),
                TermUnknownHandling::AbortEvaluation => {
                    verdict.terms.push(TermResult {
                        term_id: term.term_id.clone(),
                        outcome: TermOutcome::UnknownDropped { reason },
                        weight_milli: term.weight_milli,
                        contribution_milli: 0,
                    });
                    verdict.status = CandidateStatus::UnknownDropped;
                    let detail = format!(
                        "candidate `{}` term `{}`: {reason:?}",
                        candidate.candidate_id, term.term_id
                    );
                    verdict.reasons.push(DecisionReason::rule(
                        SelectionPolicyCode::TermUnknownAborted,
                        &term.term_id,
                        &detail,
                    ));
                    return Ok(Assessment::Aborted {
                        verdict,
                        reason: (reason, detail),
                    });
                }
            },
        };
        let transformed_milli = match &outcome {
            TermOutcome::Scored { transformed_milli }
            | TermOutcome::UnknownSubstituted {
                transformed_milli, ..
            } => *transformed_milli,
            TermOutcome::UnknownDropped { .. } => 0,
        };
        let contribution_milli = if keep {
            let product = i128::from(transformed_milli) * i128::from(term.weight_milli) / 1_000;
            i64::try_from(product).map_err(|_| {
                SelectionError::new(
                    SelectionErrorCode::ArithmeticOverflow,
                    format!("term `{}` overflows a 64-bit milli score", term.term_id),
                )
            })?
        } else {
            0
        };
        if matches!(outcome, TermOutcome::UnknownSubstituted { .. }) {
            verdict.reasons.push(DecisionReason::rule(
                SelectionPolicyCode::TermUnknownSubstituted,
                &term.term_id,
                format!("term `{}` used the declared value", term.term_id),
            ));
        }
        if keep && let Some(reason) = term_reads.marks.fallback_reason(&term.term_id) {
            verdict.reasons.push(reason);
        }
        verdict.terms.push(TermResult {
            term_id: term.term_id.clone(),
            outcome,
            weight_milli: term.weight_milli,
            contribution_milli,
        });
        if !keep {
            verdict.status = CandidateStatus::UnknownDropped;
            verdict.static_table_drop = static_unknown;
            verdict.reasons.push(DecisionReason::rule(
                SelectionPolicyCode::TermUnknownDropped,
                &term.term_id,
                format!("term `{}` met an unknown input", term.term_id),
            ));
            return Ok(Assessment::Settled {
                verdict,
                ranked: None,
            });
        }
        total += i128::from(contribution_milli);
    }

    let score_milli = i64::try_from(total).map_err(|_| {
        SelectionError::new(
            SelectionErrorCode::ArithmeticOverflow,
            format!(
                "candidate `{}` overflows a 64-bit milli score",
                candidate.candidate_id
            ),
        )
    })?;
    verdict.score_milli = Some(score_milli);
    verdict.reasons.push(DecisionReason::new(
        SelectionPolicyCode::CandidateScored,
        format!("score_milli={score_milli}"),
    ));
    let mut keys = Vec::with_capacity(policy.tie_break.len());
    for (index, key) in policy.tie_break.iter().enumerate() {
        let resolved = match key {
            TieBreakKey::CandidateId { direction } => (
                Ok(ScalarValue::String(candidate.candidate_id.clone())),
                *direction,
            ),
            TieBreakKey::Value { value, direction } => {
                (resolver.read(value, candidate), *direction)
            }
        };
        if let Err(reason) = &resolved.0 {
            verdict.reasons.push(DecisionReason::new(
                SelectionPolicyCode::TieBreakUnknown,
                format!("tie-break key {index} is unknown ({reason:?}) and orders last"),
            ));
        }
        keys.push(resolved);
    }
    Ok(Assessment::Settled {
        ranked: Some(Ranked {
            candidate_id: candidate.candidate_id.clone(),
            score_milli,
            keys,
        }),
        verdict,
    })
}

fn truth(
    predicate: &crate::schema::Predicate,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
    reads: &mut Reads,
) -> Truth {
    use crate::schema::Predicate;
    match predicate {
        Predicate::IntegerAtLeast { value, threshold } => {
            leaf(value, candidate, resolver, reads, |value| match value {
                ScalarValue::Integer(value) => Some(*value >= *threshold),
                _ => None,
            })
        }
        Predicate::IntegerAtMost { value, threshold } => {
            leaf(value, candidate, resolver, reads, |value| match value {
                ScalarValue::Integer(value) => Some(*value <= *threshold),
                _ => None,
            })
        }
        Predicate::IntegerEquals { value, expected } => {
            leaf(value, candidate, resolver, reads, |value| match value {
                ScalarValue::Integer(value) => Some(*value == *expected),
                _ => None,
            })
        }
        Predicate::BooleanEquals { value, expected } => {
            leaf(value, candidate, resolver, reads, |value| match value {
                ScalarValue::Boolean(value) => Some(*value == *expected),
                _ => None,
            })
        }
        Predicate::StringIn { value, allowed } => {
            leaf(value, candidate, resolver, reads, |value| match value {
                ScalarValue::String(value) => Some(allowed.iter().any(|member| member == value)),
                _ => None,
            })
        }
        Predicate::IntegerCompare {
            left,
            op,
            right,
            mul,
            add,
        } => {
            // Both sides are read, so the collect option lists each one that is unknown; the
            // left side's reason comes first, as a value test reports it.
            let left = integer_side(left, candidate, resolver, reads);
            let right = integer_side(right, candidate, resolver, reads);
            match (left, right) {
                (Err(reason), _) | (Ok(_), Err(reason)) => Truth::Unknown(reason),
                (Ok(left), Ok(right)) => {
                    let scale = |value: i64| {
                        i128::from(value) * i128::from(mul.unwrap_or(1))
                            + i128::from(add.unwrap_or(0))
                    };
                    let ranged = left.is_at_least() || right.is_at_least();
                    match compare_sides(*op, left, right, scale) {
                        Some(holds) => {
                            reads.marks.settled_on_bound |= ranged;
                            bool_truth(holds)
                        }
                        // Workflow #428 S168-LB1b: a range whose values disagree settles
                        // nothing; the existing unknown reason says so, and it is no static
                        // table drop.
                        None => {
                            reads.marks.other_unknown = true;
                            Truth::Unknown(UnknownReason::LookupMiss)
                        }
                    }
                }
            }
        }
        Predicate::All { of } => {
            let start = reads.unknown.len();
            let mut unknown = None;
            for inner in of {
                match truth(inner, candidate, resolver, reads) {
                    Truth::False => {
                        reads.unknown.truncate(start);
                        return Truth::False;
                    }
                    Truth::Unknown(reason) => unknown = unknown.or(Some(reason)),
                    Truth::True => {}
                }
            }
            unknown.map_or(Truth::True, Truth::Unknown)
        }
        Predicate::Any { of } => {
            let start = reads.unknown.len();
            let mut unknown = None;
            for inner in of {
                match truth(inner, candidate, resolver, reads) {
                    Truth::True => {
                        reads.unknown.truncate(start);
                        return Truth::True;
                    }
                    Truth::Unknown(reason) => unknown = unknown.or(Some(reason)),
                    Truth::False => {}
                }
            }
            unknown.map_or(Truth::False, Truth::Unknown)
        }
        Predicate::Not { of } => match truth(of, candidate, resolver, reads) {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown(reason) => Truth::Unknown(reason),
        },
    }
}

/// Decides one comparison: `decide` answers for a value of the type it compares, and `None`
/// for any other type, which is a type mismatch.
fn leaf(
    reference: &ValueRef,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
    reads: &mut Reads,
    decide: impl FnOnce(&ScalarValue) -> Option<bool>,
) -> Truth {
    let decided = resolver
        .read_noting(reference, candidate, reads)
        .and_then(|value| decide(&value).ok_or(UnknownReason::TypeMismatch));
    match decided {
        Ok(holds) => {
            reads.record_known(resolver, reference);
            bool_truth(holds)
        }
        Err(reason) => {
            reads.record_unknown(reference, reason);
            Truth::Unknown(reason)
        }
    }
}

/// One side of an `integer_compare`: one integer, or, for a list maximum read as a lower bound
/// (Workflow #428 S168-LB1b), every integer from L up.
#[derive(Debug, Clone, Copy)]
enum Side {
    Exact(i64),
    AtLeast(i64),
}

impl Side {
    fn is_at_least(self) -> bool {
        matches!(self, Self::AtLeast(_))
    }
}

/// Reads one side of an `integer_compare`: an integer value, the range [L, +∞) of a list
/// maximum read as a lower bound, or the reason it is unknown (any other type is a type
/// mismatch).
fn integer_side(
    reference: &ValueRef,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
    reads: &mut Reads,
) -> Result<Side, UnknownReason> {
    let read = resolver
        .read_noting(reference, candidate, reads)
        .and_then(|value| match value {
            ScalarValue::Integer(value) if resolver.reads_at_least(reference) => {
                Ok(Side::AtLeast(value))
            }
            ScalarValue::Integer(value) => Ok(Side::Exact(value)),
            _ => Err(UnknownReason::TypeMismatch),
        });
    match read {
        Ok(_) => reads.record_known(resolver, reference),
        Err(reason) => reads.record_unknown(reference, reason),
    }
    read
}

/// Decides `left op right × mul + add` (`scale` applies `mul` and `add`), computed and compared
/// in 128 bits. Workflow #428 S168-LB1b: a side that is a range [L, +∞) settles the comparison
/// only where every value of the range gives the same answer, and `None` stands for a range
/// whose values disagree. On the right, `mul` is at least one, so the range is
/// [L × `mul` + `add`, +∞). Two ranges settle nothing.
fn compare_sides(
    op: IntegerCompareOp,
    left: Side,
    right: Side,
    scale: impl Fn(i64) -> i128,
) -> Option<bool> {
    match (left, right) {
        (Side::Exact(left), Side::Exact(right)) => Some(op.holds(i128::from(left), scale(right))),
        // Every value of the left side is at least `low`.
        (Side::AtLeast(low), Side::Exact(right)) => {
            let (low, right) = (i128::from(low), scale(right));
            match op {
                IntegerCompareOp::Ge => (low >= right).then_some(true),
                IntegerCompareOp::Gt | IntegerCompareOp::Ne => (low > right).then_some(true),
                IntegerCompareOp::Le | IntegerCompareOp::Eq => (low > right).then_some(false),
                IntegerCompareOp::Lt => (low >= right).then_some(false),
            }
        }
        // Every value of the right side is at least `low`.
        (Side::Exact(left), Side::AtLeast(low)) => {
            let (left, low) = (i128::from(left), scale(low));
            match op {
                IntegerCompareOp::Le => (left <= low).then_some(true),
                IntegerCompareOp::Lt | IntegerCompareOp::Ne => (left < low).then_some(true),
                IntegerCompareOp::Ge | IntegerCompareOp::Eq => (left < low).then_some(false),
                IntegerCompareOp::Gt => (left <= low).then_some(false),
            }
        }
        (Side::AtLeast(_), Side::AtLeast(_)) => None,
    }
}

/// A term's transform over a list maximum read as a lower bound, the range [L, +∞) (Workflow
/// #428 S168-LB1b): `identity` takes L; `threshold` gives `then_milli` once L reaches
/// `at_least`, and is Unknown (`lookup_miss`) otherwise, which the term's `on_unknown` decides.
/// Validation admits no lookup over a list maximum.
fn transform_at_least(transform: &Transform, value: &ScalarValue) -> Result<i64, UnknownReason> {
    match (transform, value) {
        (Transform::Identity, ScalarValue::Integer(low)) => Ok(*low),
        (
            Transform::Threshold {
                at_least,
                then_milli,
                ..
            },
            ScalarValue::Integer(low),
        ) => {
            if low >= at_least {
                Ok(*then_milli)
            } else {
                Err(UnknownReason::LookupMiss)
            }
        }
        _ => Err(UnknownReason::TypeMismatch),
    }
}

fn bool_truth(value: bool) -> Truth {
    if value { Truth::True } else { Truth::False }
}

pub(crate) fn transform(transform: &Transform, value: &ScalarValue) -> Result<i64, UnknownReason> {
    match transform {
        Transform::Identity => match value {
            ScalarValue::Integer(value) => Ok(*value),
            _ => Err(UnknownReason::TypeMismatch),
        },
        Transform::Threshold {
            at_least,
            then_milli,
            otherwise_milli,
        } => match value {
            ScalarValue::Integer(value) => Ok(if value >= at_least {
                *then_milli
            } else {
                *otherwise_milli
            }),
            _ => Err(UnknownReason::TypeMismatch),
        },
        Transform::Lookup {
            entries,
            default_milli,
        } => {
            let wanted = match value {
                ScalarValue::Integer(value) => LookupKey::Integer(*value),
                ScalarValue::Boolean(value) => LookupKey::Boolean(*value),
                ScalarValue::String(value) => LookupKey::String(value.clone()),
            };
            entries
                .iter()
                .find(|entry| entry.key == wanted)
                .map(|entry| entry.value_milli)
                .or(*default_milli)
                .ok_or(UnknownReason::LookupMiss)
        }
    }
}

fn compare(left: &Ranked, right: &Ranked) -> Ordering {
    right
        .score_milli
        .cmp(&left.score_milli)
        .then_with(|| compare_keys(left, right))
        .then_with(|| left.candidate_id.cmp(&right.candidate_id))
}

/// Orders two survivors under the declared tie-break keys.
///
/// A key whose value is unknown for one candidate and known for the other orders the known
/// candidate first, in either direction: an unknown never stands in for a value, so it can
/// never outrank one. Two unknowns under the same key are indistinguishable and fall through
/// to the next key. Skipping the key for the pair instead would make the comparison depend on
/// which pair is compared, so the listing order would decide the ranking.
fn compare_keys(left: &Ranked, right: &Ranked) -> Ordering {
    for (index, (value, direction)) in left.keys.iter().enumerate() {
        let Some((other, _)) = right.keys.get(index) else {
            break;
        };
        let ordering = match (value, other) {
            (Ok(value), Ok(other)) => match direction {
                SortDirection::HighestFirst => other.cmp(value),
                SortDirection::LowestFirst => value.cmp(other),
            },
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => Ordering::Equal,
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn tied(left: &Ranked, right: &Ranked) -> bool {
    left.score_milli == right.score_milli && compare_keys(left, right) == Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::SelectionFactEntry;
    use crate::schema::{
        FieldDeclaration, LookupEntry, Predicate, SelectionRequirement, TermUnknownHandling,
    };

    const NOW: u64 = 1_000_500;
    const FACT_KEY: &str = "activity-a.slots_free";

    fn policy() -> SelectionPolicy {
        serde_json::from_str(include_str!("../tests/fixtures/policy.json"))
            .expect("fixture policy decodes")
    }

    fn candidates() -> Vec<Candidate> {
        #[derive(serde::Deserialize)]
        struct Set {
            candidates: Vec<Candidate>,
        }
        serde_json::from_str::<Set>(include_str!("../tests/fixtures/candidates.json"))
            .expect("fixture candidates decode")
            .candidates
    }

    fn facts() -> SelectionFactSnapshot {
        serde_json::from_str(include_str!("../tests/fixtures/facts.json"))
            .expect("fixture facts decode")
    }

    fn verdict<'a>(decision: &'a SelectionDecision, candidate_id: &str) -> &'a CandidateVerdict {
        decision
            .candidates
            .iter()
            .find(|verdict| verdict.candidate_id == candidate_id)
            .expect("candidate verdict")
    }

    #[test]
    fn the_golden_fixture_selects_the_declared_top_two_with_a_full_breakdown() {
        let decision = evaluate(&policy(), &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 2 });
        assert_eq!(decision.outcome_key, "outcome-selected");
        assert_eq!(decision.selected, ["slot-1", "slot-2"]);
        assert_eq!(decision.candidate_layout_id, "layout-a");
        assert_eq!(decision.fact_snapshot_id, "snapshot-a");

        let first = verdict(&decision, "slot-1");
        assert_eq!(first.status, CandidateStatus::Ranked);
        assert_eq!(first.score_milli, Some(5_200));
        assert_eq!(first.rank, Some(1));
        assert_eq!(
            first
                .terms
                .iter()
                .map(|term| term.contribution_milli)
                .collect::<Vec<_>>(),
            [1_200, 3_000, 1_000]
        );

        let rejected = verdict(&decision, "slot-3");
        assert_eq!(rejected.status, CandidateStatus::GateRejected);
        assert_eq!(rejected.score_milli, None);
        assert_eq!(rejected.rank, None);
        assert!(rejected.terms.is_empty());
        assert_eq!(
            rejected.gates,
            [GateResult {
                gate_id: "gate-ready".to_owned(),
                outcome: GateOutcome::Failed,
            }]
        );
        assert!(
            rejected
                .reasons
                .iter()
                .any(|reason| reason.outcome.code().as_str() == "gate_rejected")
        );

        // The fourth candidate ties the second on score and on the first tie-break key; the
        // declared identifier key is what separates them.
        assert_eq!(verdict(&decision, "slot-4").rank, Some(3));
    }

    #[test]
    fn the_same_input_yields_the_same_decision_and_the_same_identity() {
        let first = evaluate(&policy(), &candidates(), &facts(), NOW).expect("decision");
        let second = evaluate(&policy(), &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(first, second);
        assert_eq!(
            crate::canonical_sha256(&first).expect("identity"),
            crate::canonical_sha256(&second).expect("identity")
        );
        assert_eq!(
            first.policy_sha256,
            crate::canonical_sha256(&policy()).expect("identity")
        );
    }

    /// The instance snapshot of the input identity golden: the fixture's scalar fact and one
    /// record list that the fixture document does not declare.
    fn snapshot_with_an_undeclared_list() -> actingcommand_contract::InstanceFactSnapshot {
        use actingcommand_contract::{
            FactContent, FactRecord, FactScalar, FactScope, FactValue, InstanceFactContext,
        };
        let record = |key: &str, value: FactValue, expires_at_unix_ms: Option<u64>| FactRecord {
            scope: FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            key: key.to_owned(),
            content: FactContent::Inline { value },
            observed_at_unix_ms: 1_000_000,
            expires_at_unix_ms,
            ttl_policy: None,
            confidence_milli: 900,
            source_detector: "detector-a".to_owned(),
            source_snapshot_id: "snapshot-a".to_owned(),
            schema_version: "v1".to_owned(),
            resource_bundle_hash: "0".repeat(64),
            invalidate_on: Vec::new(),
        };
        actingcommand_contract::InstanceFactSnapshot {
            snapshot_id: "snapshot-a".to_owned(),
            ledger_position: 7,
            context: InstanceFactContext {
                instance_id: "instance-a".to_owned(),
                server_id: "server-a".to_owned(),
                game_id: "game-a".to_owned(),
            },
            records: vec![
                record(FACT_KEY, FactValue::Integer(3), Some(4_600_000)),
                record(
                    "session.example.list.targets",
                    FactValue::RecordList(vec![BTreeMap::from([
                        ("id".to_owned(), FactScalar::String("member-a".to_owned())),
                        ("rank".to_owned(), FactScalar::Integer(2)),
                    ])]),
                    None,
                ),
            ],
        }
    }

    /// The `input_sha256` of a document that reads no record list, over an instance snapshot
    /// that holds one: the undeclared list stays `fact_not_scalar`, so the hashed input keeps
    /// its bytes.
    const INPUT_IDENTITY_GOLDEN: &str =
        "sha256:77ee97e6e9440f13da5be07eefb2e900f5f1c59944e35e4e43a4ad0efc6a73f0";

    #[test]
    fn a_document_without_keyed_fact_keeps_its_input_identity() {
        let facts =
            SelectionFactSnapshot::from_instance_snapshot(&snapshot_with_an_undeclared_list(), NOW);
        let decision = evaluate(&policy(), &candidates(), &facts, NOW).expect("decision");
        assert_eq!(decision.input_sha256, INPUT_IDENTITY_GOLDEN);

        // The select step builds the snapshot with the lists its document declares: none here.
        let policy = policy();
        let with_lists = SelectionFactSnapshot::from_instance_snapshot_with_lists(
            &snapshot_with_an_undeclared_list(),
            NOW,
            &policy.record_list_keys(),
        );
        assert_eq!(with_lists, facts);
        let decision = evaluate(&policy, &candidates(), &with_lists, NOW).expect("decision");
        assert_eq!(decision.input_sha256, INPUT_IDENTITY_GOLDEN);
    }

    const LIST_KEY: &str = "session.example.list.targets";

    fn list_row(id: &str, rank: i64) -> BTreeMap<String, actingcommand_contract::FactScalar> {
        use actingcommand_contract::FactScalar;
        BTreeMap::from([
            ("id".to_owned(), FactScalar::String(id.to_owned())),
            ("rank".to_owned(), FactScalar::Integer(rank)),
        ])
    }

    /// The instance-scoped list `LIST_KEY`, published at 1 000 000 with full confidence and no
    /// TTL, as a state list is.
    fn list_record(
        rows: Vec<BTreeMap<String, actingcommand_contract::FactScalar>>,
    ) -> actingcommand_contract::FactRecord {
        use actingcommand_contract::{FactContent, FactRecord, FactScope, FactValue};
        FactRecord {
            scope: FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            key: LIST_KEY.to_owned(),
            content: FactContent::Inline {
                value: FactValue::RecordList(rows),
            },
            observed_at_unix_ms: 1_000_000,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: "decision-state".to_owned(),
            source_snapshot_id: "decision-state:list-a".to_owned(),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "0".repeat(64),
            invalidate_on: Vec::new(),
        }
    }

    fn list_facts(
        policy: &SelectionPolicy,
        records: &[actingcommand_contract::FactRecord],
    ) -> SelectionFactSnapshot {
        SelectionFactSnapshot::from_fact_records_with_lists(
            "snapshot-list",
            NOW,
            records,
            &policy.record_list_keys(),
        )
    }

    /// One candidate per member, `layout-list#00` to `#02`.
    fn list_candidates() -> Vec<Candidate> {
        ["member-a", "member-b", "member-c"]
            .iter()
            .enumerate()
            .map(|(index, member)| Candidate {
                candidate_id: format!("layout-list#{index:02}"),
                fields: BTreeMap::from([(
                    "member".to_owned(),
                    ScalarValue::String((*member).to_owned()),
                )]),
            })
            .collect()
    }

    #[test]
    fn a_keyed_fact_reads_the_listed_row_and_absent_for_an_unlisted_member() {
        let policy = crate::schema::keyed_policy();
        let facts = list_facts(
            &policy,
            &[list_record(vec![
                list_row("member-a", 2),
                list_row("member-c", 3),
            ])],
        );
        let decision = evaluate(&policy, &list_candidates(), &facts, NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 1 });
        assert_eq!(decision.selected, ["layout-list#02"]);
        assert_eq!(verdict(&decision, "layout-list#00").score_milli, Some(2));
        // No row is the known value `absent`, so the gate decides, and the member is rejected
        // rather than dropped as unknown.
        let unlisted = verdict(&decision, "layout-list#01");
        assert_eq!(unlisted.status, CandidateStatus::GateRejected);
        assert_eq!(unlisted.gates[0].outcome, GateOutcome::Failed);
    }

    #[test]
    fn an_unreadable_list_ends_the_evaluation_before_any_candidate() {
        use actingcommand_contract::{FactRecord, FactScalar};
        let policy = crate::schema::keyed_policy();
        let listed = || list_record(vec![list_row("member-a", 2), list_row("member-c", 3)]);
        let without_rank =
            BTreeMap::from([("id".to_owned(), FactScalar::String("member-c".to_owned()))]);
        let cases: Vec<(&str, Vec<FactRecord>, u64, UnknownReason)> = vec![
            ("missing", Vec::new(), NOW, UnknownReason::FactMissing),
            (
                "stale",
                vec![listed()],
                1_000_000 + 3_600_001,
                UnknownReason::FactStale,
            ),
            (
                "expired",
                vec![FactRecord {
                    expires_at_unix_ms: Some(1_000_400),
                    ..listed()
                }],
                NOW,
                UnknownReason::FactExpired,
            ),
            (
                "low confidence",
                vec![FactRecord {
                    confidence_milli: 999,
                    ..listed()
                }],
                NOW,
                UnknownReason::FactLowConfidence,
            ),
            (
                "duplicated",
                vec![list_record(vec![
                    list_row("member-a", 2),
                    list_row("member-a", 4),
                ])],
                NOW,
                UnknownReason::TypeMismatch,
            ),
            (
                "missing a column",
                vec![list_record(vec![list_row("member-a", 2), without_rank])],
                NOW,
                UnknownReason::TypeMismatch,
            ),
        ];
        for (label, records, now, reason) in cases {
            let facts = list_facts(&policy, &records);
            // Zero candidates are checked too: `empty` needs the list read first.
            for candidates in [list_candidates(), Vec::new()] {
                let decision = evaluate(&policy, &candidates, &facts, now).expect(label);
                assert!(
                    matches!(
                        &decision.outcome,
                        SelectionOutcome::Unknown { reason: actual, .. } if *actual == reason
                    ),
                    "{label}: {:?}",
                    decision.outcome
                );
                assert_eq!(decision.outcome_key, "list-unknown", "{label}");
                assert!(decision.candidates.is_empty(), "{label}");
                assert!(decision.selected.is_empty(), "{label}");
            }
        }

        let facts = list_facts(&policy, &[listed()]);
        let decision = evaluate(&policy, &[], &facts, NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Empty);
    }

    #[test]
    fn the_collect_option_refuses_a_document_that_declares_a_record_list() {
        // Only a select step reads a record list; the collect option refuses the document
        // even when the list is present and readable.
        let policy = crate::schema::keyed_policy();
        let facts = list_facts(
            &policy,
            &[list_record(vec![
                list_row("member-a", 2),
                list_row("member-c", 3),
            ])],
        );
        let error = evaluate_collect(&policy, &list_candidates(), &facts, NOW)
            .expect_err("the collect option reads no record list");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    }

    #[test]
    fn where_selects_the_rows_of_one_kind_before_keys_must_be_unique() {
        use actingcommand_contract::FactScalar;
        let mut policy = crate::schema::keyed_policy();
        if let ValueType::RecordList { columns, .. } = &mut policy.facts[0].value_type {
            columns.insert(
                "kind".to_owned(),
                ValueType::EnumString {
                    allowed: vec!["kind-x".to_owned(), "kind-y".to_owned()],
                },
            );
        }
        let row = |id: &str, kind: &str, rank: i64| {
            let mut row = list_row(id, rank);
            row.insert("kind".to_owned(), FactScalar::String(kind.to_owned()));
            row
        };
        let records = [list_record(vec![
            row("member-a", "kind-x", 2),
            row("member-a", "kind-y", 5),
            row("member-c", "kind-x", 3),
        ])];
        let decision = evaluate(
            &policy,
            &list_candidates(),
            &list_facts(&policy, &records),
            NOW,
        )
        .expect("decision");
        assert!(matches!(
            decision.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::TypeMismatch,
                ..
            }
        ));

        let filter = Some(RowFilter {
            column: "kind".to_owned(),
            equals: "kind-x".to_owned(),
        });
        let set_filter = |value: &mut ValueRef| {
            if let ValueRef::KeyedFact { row_filter, .. } = value {
                *row_filter = filter.clone();
            }
        };
        set_filter(&mut policy.scoring[0].value);
        if let Predicate::IntegerAtLeast { value, .. } = &mut policy.gates[0].predicate {
            set_filter(value);
        }
        policy.validate().expect("filtered policy validates");
        let decision = evaluate(
            &policy,
            &list_candidates(),
            &list_facts(&policy, &records),
            NOW,
        )
        .expect("decision");
        assert_eq!(decision.selected, ["layout-list#02"]);
        assert_eq!(verdict(&decision, "layout-list#00").score_milli, Some(2));
    }

    #[test]
    fn reordering_the_candidate_set_keeps_the_same_selection() {
        let mut reordered = candidates();
        reordered.reverse();
        let ordered = evaluate(&policy(), &candidates(), &facts(), NOW).expect("decision");
        let shuffled = evaluate(&policy(), &reordered, &facts(), NOW).expect("decision");
        assert_eq!(ordered.selected, shuffled.selected);
        assert_ne!(ordered.input_sha256, shuffled.input_sha256);
    }

    #[test]
    fn an_unknown_fact_drops_candidates_instead_of_scoring_it_as_zero() {
        let mut snapshot = facts();
        snapshot.facts.clear();
        let decision = evaluate(&policy(), &candidates(), &snapshot, NOW).expect("decision");
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Insufficient {
                surviving: 0,
                required: 2,
            }
        );
        assert!(decision.selected.is_empty());
        let dropped = verdict(&decision, "slot-1");
        assert_eq!(dropped.status, CandidateStatus::UnknownDropped);
        assert_eq!(
            dropped.terms.last().expect("capacity term").outcome,
            TermOutcome::UnknownDropped {
                reason: UnknownReason::FactMissing,
            }
        );
        // A zero-valued fact would have scored the low side of the threshold for the same
        // term; an unknown one does not score at all.
        let mut zeroed = facts();
        if let Some(SelectionFactEntry::Published { value, .. }) = zeroed.facts.get_mut(FACT_KEY) {
            *value = ScalarValue::Integer(0);
        }
        let zeroed = evaluate(&policy(), &candidates(), &zeroed, NOW).expect("decision");
        assert_eq!(verdict(&zeroed, "slot-1").score_milli, Some(3_700));
    }

    #[test]
    fn an_expired_fact_is_unknown_rather_than_stale_data() {
        let snapshot = facts();
        // One millisecond past the record's own expiry; the expiry instant itself still reads.
        let decision = evaluate(&policy(), &candidates(), &snapshot, 4_600_001).expect("decision");
        assert_eq!(
            verdict(&decision, "slot-1")
                .terms
                .last()
                .expect("capacity term")
                .outcome,
            TermOutcome::UnknownDropped {
                reason: UnknownReason::FactExpired,
            }
        );
    }

    #[test]
    fn a_declared_substitution_is_used_and_recorded() {
        let mut policy = policy();
        policy.scoring[2].on_unknown = TermUnknownHandling::SubstituteMilli { value_milli: 250 };
        let mut snapshot = facts();
        snapshot.facts.clear();
        let decision = evaluate(&policy, &candidates(), &snapshot, NOW).expect("decision");
        let first = verdict(&decision, "slot-1");
        assert_eq!(
            first.terms.last().expect("capacity term").outcome,
            TermOutcome::UnknownSubstituted {
                reason: UnknownReason::FactMissing,
                transformed_milli: 250,
            }
        );
        assert_eq!(first.score_milli, Some(1_200 + 3_000 + 250));
        assert!(
            first
                .reasons
                .iter()
                .any(|reason| reason.outcome.code().as_str() == "term_unknown_substituted")
        );
    }

    #[test]
    fn an_aborting_rule_ends_the_evaluation_with_an_unknown_outcome() {
        let mut policy = policy();
        policy.scoring[2].on_unknown = TermUnknownHandling::AbortEvaluation;
        let mut snapshot = facts();
        snapshot.facts.clear();
        let decision = evaluate(&policy, &candidates(), &snapshot, NOW).expect("decision");
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Unknown {
                reason: UnknownReason::FactMissing,
                detail: "candidate `slot-1` term `term-capacity`: FactMissing".to_owned(),
            }
        );
        assert_eq!(decision.outcome_key, "outcome-unknown");
        assert!(decision.selected.is_empty());
        assert_eq!(decision.candidates.len(), 1);
    }

    #[test]
    fn a_gate_that_cannot_be_decided_follows_its_declared_handling() {
        let mut policy = policy();
        policy.facts[0].value_type = ValueType::Boolean;
        policy.gates.push(crate::HardGate {
            gate_id: "gate-capacity".to_owned(),
            predicate: Predicate::BooleanEquals {
                value: ValueRef::Fact {
                    fact_key: FACT_KEY.to_owned(),
                },
                expected: true,
            },
            on_unknown: GateUnknownHandling::SubstituteVerdict { passes: false },
        });
        policy.scoring.remove(2);
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        let first = verdict(&decision, "slot-1");
        // The substituted verdict is a real verdict, so the candidate is gate-rejected, and
        // the gate result still carries the reason the substitution was needed.
        assert_eq!(first.status, CandidateStatus::GateRejected);
        assert_eq!(
            first.gates.last().expect("capacity gate").outcome,
            GateOutcome::UnknownSubstituted {
                reason: UnknownReason::TypeMismatch,
                passes: false,
            }
        );
        assert!(
            first
                .reasons
                .iter()
                .any(|reason| reason.outcome.code().as_str() == "gate_unknown_substituted")
        );

        policy.gates[1].on_unknown = GateUnknownHandling::DropCandidate;
        let dropped = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            verdict(&dropped, "slot-1").status,
            CandidateStatus::UnknownDropped
        );
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Insufficient {
                surviving: 0,
                required: 2,
            }
        );
    }

    #[test]
    fn an_unknown_member_leaves_a_conjunction_undecided_but_a_false_member_decides_it() {
        let mut policy = policy();
        policy.fields.push(FieldDeclaration {
            name: "absent".to_owned(),
            value_type: ValueType::Integer,
        });
        let unknown_member = Predicate::IntegerAtLeast {
            value: ValueRef::Field {
                field: "absent".to_owned(),
            },
            threshold: 1,
        };
        let false_member = Predicate::BooleanEquals {
            value: ValueRef::Field {
                field: "ready".to_owned(),
            },
            expected: false,
        };
        policy.gates[0].predicate = Predicate::All {
            of: vec![unknown_member.clone(), false_member],
        };
        policy.gates[0].on_unknown = GateUnknownHandling::DropCandidate;
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        // The false member settles the conjunction even though another member is unknown.
        assert_eq!(
            verdict(&decision, "slot-1").status,
            CandidateStatus::GateRejected
        );

        policy.gates[0].predicate = Predicate::Any {
            of: vec![unknown_member],
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            verdict(&decision, "slot-1").gates[0].outcome,
            GateOutcome::UnknownDropped {
                reason: UnknownReason::FieldMissing,
            }
        );
    }

    #[test]
    fn an_unbroken_tie_at_the_cut_is_ambiguous_rather_than_arbitrary() {
        let mut policy = policy();
        policy.tie_break.pop();
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Ambiguous {
                candidate_ids: vec!["slot-2".to_owned(), "slot-4".to_owned()],
            }
        );
        assert_eq!(decision.outcome_key, "outcome-ambiguous");
        assert!(decision.selected.is_empty());
        // The listing stays fully ordered even when the cut is ambiguous.
        assert_eq!(verdict(&decision, "slot-2").rank, Some(2));
        assert_eq!(verdict(&decision, "slot-4").rank, Some(3));
    }

    fn tie_break_candidate(candidate_id: &str, extra: Option<i64>) -> Candidate {
        let mut fields = BTreeMap::new();
        fields.insert("value_milli".to_owned(), ScalarValue::Integer(900));
        fields.insert("ready".to_owned(), ScalarValue::Boolean(true));
        fields.insert(
            "grade".to_owned(),
            ScalarValue::String("grade-mid".to_owned()),
        );
        if let Some(extra) = extra {
            fields.insert("extra".to_owned(), ScalarValue::Integer(extra));
        }
        Candidate {
            candidate_id: candidate_id.to_owned(),
            fields,
        }
    }

    fn tie_break_policy(direction: SortDirection) -> SelectionPolicy {
        let mut policy = policy();
        policy.fields.push(FieldDeclaration {
            name: "extra".to_owned(),
            value_type: ValueType::Integer,
        });
        policy.tie_break = vec![TieBreakKey::Value {
            value: ValueRef::Field {
                field: "extra".to_owned(),
            },
            direction,
        }];
        policy
    }

    #[test]
    fn an_unknown_tie_break_key_orders_last_whatever_the_listing_order_is() {
        let policy = tie_break_policy(SortDirection::HighestFirst);
        let known_high = tie_break_candidate("unit-1", Some(5));
        let unknown = tie_break_candidate("unit-2", None);
        let known_low = tie_break_candidate("unit-3", Some(1));
        let listings = [
            vec![known_high.clone(), unknown.clone(), known_low.clone()],
            vec![known_low.clone(), unknown.clone(), known_high.clone()],
            vec![unknown.clone(), known_high.clone(), known_low.clone()],
        ];
        for listing in &listings {
            let decision = evaluate(&policy, listing, &facts(), NOW).expect("decision");
            assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 2 });
            assert_eq!(decision.selected, ["unit-1", "unit-3"]);
            let skipped = verdict(&decision, "unit-2");
            assert_eq!(skipped.rank, Some(3));
            assert!(
                skipped
                    .reasons
                    .iter()
                    .any(|reason| reason.outcome.code().as_str() == "tie_break_unknown")
            );
        }

        // The direction orders the known values against each other and never promotes the
        // candidate that has no value for the key.
        let lowest = tie_break_policy(SortDirection::LowestFirst);
        let decision = evaluate(&lowest, &listings[0], &facts(), NOW).expect("decision");
        assert_eq!(decision.selected, ["unit-3", "unit-1"]);
        assert_eq!(verdict(&decision, "unit-2").rank, Some(3));
    }

    #[test]
    fn a_known_tie_break_key_separates_from_an_unknown_one_but_two_unknowns_do_not() {
        let mut policy = tie_break_policy(SortDirection::HighestFirst);
        policy.selection = SelectionRequirement {
            mode: SelectionMode::ExactlyOne,
            required_count: 1,
        };
        let decision = evaluate(
            &policy,
            &[
                tie_break_candidate("unit-2", None),
                tie_break_candidate("unit-4", Some(1)),
            ],
            &facts(),
            NOW,
        )
        .expect("decision");
        assert_eq!(decision.selected, ["unit-4"]);

        let decision = evaluate(
            &policy,
            &[
                tie_break_candidate("unit-2", None),
                tie_break_candidate("unit-4", None),
            ],
            &facts(),
            NOW,
        )
        .expect("decision");
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Ambiguous {
                candidate_ids: vec!["unit-2".to_owned(), "unit-4".to_owned()],
            }
        );
        assert!(decision.selected.is_empty());
    }

    #[test]
    fn reversing_a_tie_break_direction_reverses_the_choice() {
        let mut policy = policy();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::ExactlyOne,
            required_count: 1,
        };
        policy.tie_break = vec![TieBreakKey::CandidateId {
            direction: SortDirection::HighestFirst,
        }];
        policy.scoring.clear();
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.selected, ["slot-4"]);

        policy.tie_break = vec![TieBreakKey::CandidateId {
            direction: SortDirection::LowestFirst,
        }];
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.selected, ["slot-1"]);
    }

    #[test]
    fn exactly_one_takes_the_single_highest_survivor() {
        let mut policy = policy();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::ExactlyOne,
            required_count: 1,
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 1 });
        assert_eq!(decision.selected, ["slot-1"]);
    }

    #[test]
    fn top_k_reports_insufficient_instead_of_selecting_fewer() {
        let mut policy = policy();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::TopK,
            required_count: 4,
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            decision.outcome,
            SelectionOutcome::Insufficient {
                surviving: 3,
                required: 4,
            }
        );
        assert_eq!(decision.outcome_key, "outcome-insufficient");
        assert!(decision.selected.is_empty());
    }

    #[test]
    fn none_allowed_accepts_an_empty_answer_and_takes_what_it_can() {
        let mut policy = policy();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::NoneAllowed,
            required_count: 0,
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Empty);
        assert_eq!(decision.outcome_key, "outcome-empty");
        assert!(decision.selected.is_empty());

        policy.selection.required_count = 9;
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(decision.outcome, SelectionOutcome::Selected { count: 3 });
        assert_eq!(decision.selected, ["slot-1", "slot-2", "slot-4"]);
    }

    #[test]
    fn a_lookup_that_covers_nothing_is_unknown_and_a_default_answers_it() {
        let mut policy = policy();
        policy.scoring[1].transform = Transform::Lookup {
            entries: vec![LookupEntry {
                key: LookupKey::String("grade-low".to_owned()),
                value_milli: 10,
            }],
            default_milli: None,
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            verdict(&decision, "slot-1").terms[1].outcome,
            TermOutcome::UnknownSubstituted {
                reason: UnknownReason::LookupMiss,
                transformed_milli: 0,
            }
        );

        policy.scoring[1].transform = Transform::Lookup {
            entries: vec![LookupEntry {
                key: LookupKey::String("grade-low".to_owned()),
                value_milli: 10,
            }],
            default_milli: Some(20),
        };
        let decision = evaluate(&policy, &candidates(), &facts(), NOW).expect("decision");
        assert_eq!(
            verdict(&decision, "slot-1").terms[1].outcome,
            TermOutcome::Scored {
                transformed_milli: 20,
            }
        );
    }

    #[test]
    fn a_repeated_or_empty_candidate_identifier_is_an_error() {
        let mut repeated = candidates();
        repeated[1].candidate_id = repeated[0].candidate_id.clone();
        let error = evaluate(&policy(), &repeated, &facts(), NOW).expect_err("repeated id");
        assert_eq!(error.code(), SelectionErrorCode::DuplicateId);

        let mut empty = candidates();
        empty[0].candidate_id.clear();
        let error = evaluate(&policy(), &empty, &facts(), NOW).expect_err("empty id");
        assert_eq!(error.code(), SelectionErrorCode::MissingRequiredField);
    }

    #[test]
    fn an_overflowing_weight_is_an_error_rather_than_a_wrapped_score() {
        const SAFE_MAX: i64 = 9_007_199_254_740_991;
        let mut policy = policy();
        policy.scoring[0].weight_milli = SAFE_MAX;
        let mut candidates = candidates();
        candidates[0]
            .fields
            .insert("value_milli".to_owned(), ScalarValue::Integer(SAFE_MAX));
        let error = evaluate(&policy, &candidates, &facts(), NOW).expect_err("overflow");
        assert_eq!(error.code(), SelectionErrorCode::ArithmeticOverflow);

        // A weight outside the canonical integer range never reaches the arithmetic.
        policy.scoring[0].weight_milli = i64::MAX;
        let error = evaluate(&policy, &candidates, &facts(), NOW).expect_err("unsafe integer");
        assert_eq!(error.code(), SelectionErrorCode::IntegerOutOfRange);
    }
}
