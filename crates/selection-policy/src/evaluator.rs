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
//! that it did. Nothing is silently read as `false` or `0`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::canonical::canonical_sha256;
use crate::facts::{Candidate, ScalarValue, SelectionFactSnapshot, UnknownReason};
use crate::schema::{
    FactDeclaration, GateUnknownHandling, LookupKey, MAX_CANDIDATES,
    SELECTION_POLICY_SCHEMA_VERSION, SelectionError, SelectionErrorCode, SelectionMode,
    SelectionPolicy, SortDirection, TermUnknownHandling, TieBreakKey, Transform, ValueRef,
    ValueType,
};

/// One entry in the decision's reason chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReason {
    pub code: String,
    pub detail: String,
}

impl DecisionReason {
    fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
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
}

/// Evaluates one policy over one candidate set and one fact snapshot.
///
/// The returned decision holds every candidate's breakdown, the chosen identifiers, and the
/// reason chain. An `Err` means the inputs could not be evaluated at all: an invalid
/// document, a malformed candidate set, or integer overflow. An unknown input is not an
/// error; it produces a typed unknown outcome or a dropped candidate, as the document says.
pub fn evaluate(
    policy: &SelectionPolicy,
    candidates: &[Candidate],
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<SelectionDecision, SelectionError> {
    policy.validate()?;
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

    let policy_sha256 = canonical_sha256(policy)?;
    let input_sha256 = canonical_sha256(&EvaluationInput {
        candidates,
        facts,
        now_unix_ms,
    })?;
    let mut reasons = vec![
        DecisionReason::new("policy.identity", &policy_sha256),
        DecisionReason::new("input.identity", &input_sha256),
        DecisionReason::new(
            "selection.requirement",
            format!(
                "{:?} required_count={}",
                policy.selection.mode, policy.selection.required_count
            ),
        ),
    ];

    let resolver = Resolver {
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
        snapshot: facts,
        now_unix_ms,
    };

    let mut verdicts = Vec::with_capacity(candidates.len());
    let mut survivors: Vec<Ranked> = Vec::new();
    for candidate in candidates {
        match assess(policy, candidate, &resolver)? {
            Assessment::Aborted { verdict, reason } => {
                verdicts.push(verdict);
                reasons.push(DecisionReason::new("selection.unknown", &reason.1));
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
                "candidate.ranked",
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
    match &outcome {
        SelectionOutcome::Selected { count } => reasons.push(DecisionReason::new(
            "selection.selected",
            format!("{count} of {} surviving", survivors.len()),
        )),
        SelectionOutcome::Empty => reasons.push(DecisionReason::new(
            "selection.empty",
            format!("{} surviving, none required", survivors.len()),
        )),
        SelectionOutcome::Insufficient {
            surviving,
            required,
        } => reasons.push(DecisionReason::new(
            "selection.insufficient",
            format!("{surviving} surviving below required {required}"),
        )),
        SelectionOutcome::Ambiguous { candidate_ids } => reasons.push(DecisionReason::new(
            "selection.ambiguous",
            format!("tie-break leaves {} at the cut", candidate_ids.join(", ")),
        )),
        SelectionOutcome::Unknown { .. } => {}
    }
    for verdict in &mut verdicts {
        if selected.contains(&verdict.candidate_id) {
            verdict
                .reasons
                .push(DecisionReason::new("candidate.selected", "chosen"));
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
    reasons.push(DecisionReason::new("selection.outcome_key", &outcome_key));
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

struct Resolver<'a> {
    fields: BTreeMap<&'a str, &'a ValueType>,
    facts: BTreeMap<&'a str, &'a FactDeclaration>,
    snapshot: &'a SelectionFactSnapshot,
    now_unix_ms: u64,
}

impl Resolver<'_> {
    fn read(
        &self,
        reference: &ValueRef,
        candidate: &Candidate,
    ) -> Result<ScalarValue, UnknownReason> {
        match reference {
            ValueRef::Field { field } => {
                let declared = self
                    .fields
                    .get(field.as_str())
                    .ok_or(UnknownReason::FieldMissing)?;
                let value = candidate
                    .fields
                    .get(field.as_str())
                    .ok_or(UnknownReason::FieldMissing)?;
                if value.matches(declared) {
                    Ok(value.clone())
                } else {
                    Err(UnknownReason::TypeMismatch)
                }
            }
            ValueRef::Fact { fact_key } => {
                let declared = self
                    .facts
                    .get(fact_key.as_str())
                    .ok_or(UnknownReason::FactMissing)?;
                self.snapshot.resolve(declared, self.now_unix_ms).cloned()
            }
        }
    }
}

struct Ranked {
    candidate_id: String,
    score_milli: i64,
    keys: Vec<(Option<ScalarValue>, SortDirection)>,
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

fn assess(
    policy: &SelectionPolicy,
    candidate: &Candidate,
    resolver: &Resolver<'_>,
) -> Result<Assessment, SelectionError> {
    let mut verdict = CandidateVerdict {
        candidate_id: candidate.candidate_id.clone(),
        status: CandidateStatus::Ranked,
        score_milli: None,
        rank: None,
        gates: Vec::new(),
        terms: Vec::new(),
        reasons: Vec::new(),
    };

    for gate in &policy.gates {
        let (outcome, keep) = match truth(&gate.predicate, candidate, resolver) {
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
                    verdict
                        .reasons
                        .push(DecisionReason::new("gate.unknown_abort", &detail));
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
            verdict.reasons.push(DecisionReason::new(
                "gate.unknown_substituted",
                format!("gate `{}` used the declared verdict", gate.gate_id),
            ));
        }
        if !keep {
            verdict.status = if dropped {
                verdict.reasons.push(DecisionReason::new(
                    "gate.unknown_dropped",
                    format!("gate `{}` met an unknown input", gate.gate_id),
                ));
                CandidateStatus::UnknownDropped
            } else {
                verdict.reasons.push(DecisionReason::new(
                    "gate.rejected",
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
    for term in &policy.scoring {
        let resolved = resolver
            .read(&term.value, candidate)
            .and_then(|value| transform(&term.transform, &value));
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
                    verdict
                        .reasons
                        .push(DecisionReason::new("term.unknown_abort", &detail));
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
            verdict.reasons.push(DecisionReason::new(
                "term.unknown_substituted",
                format!("term `{}` used the declared value", term.term_id),
            ));
        }
        verdict.terms.push(TermResult {
            term_id: term.term_id.clone(),
            outcome,
            weight_milli: term.weight_milli,
            contribution_milli,
        });
        if !keep {
            verdict.status = CandidateStatus::UnknownDropped;
            verdict.reasons.push(DecisionReason::new(
                "term.unknown_dropped",
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
        "candidate.scored",
        format!("score_milli={score_milli}"),
    ));
    let keys = policy
        .tie_break
        .iter()
        .map(|key| match key {
            TieBreakKey::CandidateId { direction } => (
                Some(ScalarValue::String(candidate.candidate_id.clone())),
                *direction,
            ),
            TieBreakKey::Value { value, direction } => {
                (resolver.read(value, candidate).ok(), *direction)
            }
        })
        .collect();
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
) -> Truth {
    use crate::schema::Predicate;
    let integer = |reference: &ValueRef| match resolver.read(reference, candidate) {
        Ok(ScalarValue::Integer(value)) => Ok(value),
        Ok(_) => Err(UnknownReason::TypeMismatch),
        Err(reason) => Err(reason),
    };
    match predicate {
        Predicate::IntegerAtLeast { value, threshold } => match integer(value) {
            Ok(value) => bool_truth(value >= *threshold),
            Err(reason) => Truth::Unknown(reason),
        },
        Predicate::IntegerAtMost { value, threshold } => match integer(value) {
            Ok(value) => bool_truth(value <= *threshold),
            Err(reason) => Truth::Unknown(reason),
        },
        Predicate::IntegerEquals { value, expected } => match integer(value) {
            Ok(value) => bool_truth(value == *expected),
            Err(reason) => Truth::Unknown(reason),
        },
        Predicate::BooleanEquals { value, expected } => match resolver.read(value, candidate) {
            Ok(ScalarValue::Boolean(value)) => bool_truth(value == *expected),
            Ok(_) => Truth::Unknown(UnknownReason::TypeMismatch),
            Err(reason) => Truth::Unknown(reason),
        },
        Predicate::StringIn { value, allowed } => match resolver.read(value, candidate) {
            Ok(ScalarValue::String(value)) => {
                bool_truth(allowed.iter().any(|member| member == &value))
            }
            Ok(_) => Truth::Unknown(UnknownReason::TypeMismatch),
            Err(reason) => Truth::Unknown(reason),
        },
        Predicate::All { of } => {
            let mut unknown = None;
            for inner in of {
                match truth(inner, candidate, resolver) {
                    Truth::False => return Truth::False,
                    Truth::Unknown(reason) => unknown = unknown.or(Some(reason)),
                    Truth::True => {}
                }
            }
            unknown.map_or(Truth::True, Truth::Unknown)
        }
        Predicate::Any { of } => {
            let mut unknown = None;
            for inner in of {
                match truth(inner, candidate, resolver) {
                    Truth::True => return Truth::True,
                    Truth::Unknown(reason) => unknown = unknown.or(Some(reason)),
                    Truth::False => {}
                }
            }
            unknown.map_or(Truth::False, Truth::Unknown)
        }
        Predicate::Not { of } => match truth(of, candidate, resolver) {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown(reason) => Truth::Unknown(reason),
        },
    }
}

fn bool_truth(value: bool) -> Truth {
    if value { Truth::True } else { Truth::False }
}

fn transform(transform: &Transform, value: &ScalarValue) -> Result<i64, UnknownReason> {
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

fn compare_keys(left: &Ranked, right: &Ranked) -> Ordering {
    for (index, (value, direction)) in left.keys.iter().enumerate() {
        let Some((other, _)) = right.keys.get(index) else {
            break;
        };
        let (Some(value), Some(other)) = (value.as_ref(), other.as_ref()) else {
            continue;
        };
        let ordering = match direction {
            SortDirection::HighestFirst => other.cmp(value),
            SortDirection::LowestFirst => value.cmp(other),
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
                .any(|reason| reason.code == "gate.rejected")
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
        let decision = evaluate(&policy(), &candidates(), &snapshot, 4_600_000).expect("decision");
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
                .any(|reason| reason.code == "term.unknown_substituted")
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
                .any(|reason| reason.code == "gate.unknown_substituted")
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

    #[test]
    fn pure_evaluator_source_has_no_runtime_side_effect_authority() {
        const SOURCES: &[(&str, &str)] = &[
            ("lib.rs", include_str!("lib.rs")),
            ("canonical.rs", include_str!("canonical.rs")),
            ("evaluator.rs", include_str!("evaluator.rs")),
            ("facts.rs", include_str!("facts.rs")),
            ("schema.rs", include_str!("schema.rs")),
        ];
        for (name, source) in SOURCES {
            let production = source
                .split("#[cfg(test)]")
                .next()
                .expect("production source");
            for forbidden in [
                "std::thread::sleep",
                "std::fs",
                "std::net",
                "std::process",
                "SystemTime::now",
                "Instant::now",
                "actingcommand_device",
                "actingcommand_ledger",
                "LeaseToken",
            ] {
                assert!(
                    !production.contains(forbidden),
                    "{name} holds forbidden source token {forbidden}"
                );
            }
        }
    }
}
