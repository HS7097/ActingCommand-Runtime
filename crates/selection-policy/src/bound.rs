// SPDX-License-Identifier: AGPL-3.0-only

//! The upper bound of what a candidate that was not read could score.
//!
//! [`upper_bound`] takes no candidate. For each hypothetical identity it bounds the score a
//! candidate of that identity could reach under one document: identity fields, scalar facts
//! and `keyed_fact` values are exact for that identity, and every other field takes every value
//! of its declared domain. A caller compares the bound with a chosen score to tell whether a
//! candidate it could not read might outrank the choice. Like the evaluator, it reads no clock,
//! ledger, pack or file.

use std::collections::BTreeMap;

use crate::evaluator::{ListIndex, read_lists, transform};
use crate::facts::{ScalarValue, SelectionFactSnapshot};
use crate::schema::{
    FactDeclaration, LookupKey, Predicate, ScoringTerm, SelectionError, SelectionErrorCode,
    SelectionPolicy, TermUnknownHandling, Transform, ValueRef, ValueType,
};

/// What one candidate field can hold on a candidate that was not read. The document types a
/// field only as integer, boolean or enumerated string, so the caller states the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldDomain {
    /// An `enum_string` field that carries the hypothetical identity itself, exactly.
    Identity,
    /// A `boolean` field: either value.
    Boolean,
    /// An `integer` field: every integer in `min..=max`.
    Integer { min: i64, max: i64 },
}

/// The largest score in milli that any hypothetical identity could reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpperBound {
    /// No hypothetical identity can be ranked: a gate is false over the whole domain, or a term
    /// drops the candidate on an input that is exact and unknown.
    NoneEligible,
    /// The largest score.
    Max(i64),
    /// No finite bound: the arithmetic leaves the 64-bit range, or an input the score depends
    /// on is exact and unknown under `abort_evaluation`, or a declared list cannot be read.
    Unbounded,
}

/// The upper bound of the score a candidate of any of `identities` could reach under
/// `policy`, with the instance facts `facts` at `now_unix_ms`.
///
/// `field_domains` names the domain of every declared field. An `enum_string` field must be
/// [`FieldDomain::Identity`], and each of `identities` must be a member of its enum. Every other
/// field takes every value of its domain.
///
/// A gate excludes an identity only when it is false for every value of those domains; a gate
/// that is true or unknown for some value may pass. Each term contributes the largest of its
/// possible contributions, with the sign of its weight: an `identity` transform at the domain's
/// extremes, a `threshold` at whichever of its two values the domain reaches, a `lookup` at the
/// entries the domain reaches and its default; a `substitute_milli` value joins the candidates
/// whenever the input is ranged or unknown. An `Err` means the inputs do not describe the
/// document: an invalid document or a missing or mistyped domain.
pub fn upper_bound(
    policy: &SelectionPolicy,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
    identities: &[String],
    field_domains: &BTreeMap<String, FieldDomain>,
) -> Result<UpperBound, SelectionError> {
    policy.validate()?;
    for declaration in &policy.fields {
        let name = &declaration.name;
        let domain = field_domains.get(name).ok_or_else(|| {
            SelectionError::new(
                SelectionErrorCode::MissingRequiredField,
                format!("field={name} field_domain"),
            )
        })?;
        match (domain, &declaration.value_type) {
            (FieldDomain::Identity, ValueType::EnumString { allowed }) => {
                if let Some(identity) = identities
                    .iter()
                    .find(|identity| !allowed.contains(identity))
                {
                    return Err(SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        format!("field={name} identity={identity}"),
                    ));
                }
            }
            (FieldDomain::Boolean, ValueType::Boolean) => {}
            (FieldDomain::Integer { min, max }, ValueType::Integer) if min <= max => {}
            _ => {
                return Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("field={name} field_domain"),
                ));
            }
        }
    }
    // A list that cannot be read ends every evaluation, so nothing bounds the score.
    let Ok(lists) = read_lists(policy, facts, now_unix_ms) else {
        return Ok(UpperBound::Unbounded);
    };
    let facts_by_key: BTreeMap<&str, &FactDeclaration> = policy
        .facts
        .iter()
        .map(|declaration| (declaration.fact_key.as_str(), declaration))
        .collect();
    let mut best: Option<i64> = None;
    for identity in identities {
        let hypothesis = Hypothesis {
            identity,
            field_domains,
            facts: &facts_by_key,
            snapshot: facts,
            lists: &lists,
            now_unix_ms,
        };
        match hypothesis.score(policy) {
            Score::Excluded => {}
            Score::Unbounded => return Ok(UpperBound::Unbounded),
            Score::Max(score) => best = Some(best.map_or(score, |best| best.max(score))),
        }
    }
    Ok(best.map_or(UpperBound::NoneEligible, UpperBound::Max))
}

enum Score {
    Excluded,
    Max(i64),
    Unbounded,
}

/// What a reference reads for one hypothetical identity.
enum Reading {
    /// One exact value; `None` when it is unknown.
    Exact(Option<ScalarValue>),
    /// Either boolean value.
    AnyBoolean,
    /// Every integer in the range.
    Range(i64, i64),
}

/// Which truth values a predicate can take over the domains.
#[derive(Debug, Clone, Copy)]
struct Possible {
    truth: bool,
    falsity: bool,
    unknown: bool,
}

impl Possible {
    const UNKNOWN: Self = Self {
        truth: false,
        falsity: false,
        unknown: true,
    };

    fn exact(value: bool) -> Self {
        Self {
            truth: value,
            falsity: !value,
            unknown: false,
        }
    }
}

struct Hypothesis<'a> {
    identity: &'a str,
    field_domains: &'a BTreeMap<String, FieldDomain>,
    facts: &'a BTreeMap<&'a str, &'a FactDeclaration>,
    snapshot: &'a SelectionFactSnapshot,
    lists: &'a ListIndex<'a>,
    now_unix_ms: u64,
}

impl Hypothesis<'_> {
    fn score(&self, policy: &SelectionPolicy) -> Score {
        for gate in &policy.gates {
            let possible = self.possible(&gate.predicate);
            if !possible.truth && !possible.unknown {
                return Score::Excluded;
            }
        }
        let mut total: i128 = 0;
        for term in &policy.scoring {
            let outputs = match self.outputs(term) {
                Ok(outputs) if outputs.is_empty() => return Score::Excluded,
                Ok(outputs) => outputs,
                Err(()) => return Score::Unbounded,
            };
            let mut largest: Option<i128> = None;
            for output in outputs {
                let contribution = i128::from(output) * i128::from(term.weight_milli) / 1_000;
                if i64::try_from(contribution).is_err() {
                    return Score::Unbounded;
                }
                largest = Some(largest.map_or(contribution, |largest| largest.max(contribution)));
            }
            total += largest.unwrap_or_default();
        }
        i64::try_from(total).map_or(Score::Unbounded, Score::Max)
    }

    fn read(&self, reference: &ValueRef) -> Reading {
        match reference {
            ValueRef::Field { field } => match self.field_domains.get(field) {
                Some(FieldDomain::Identity) => {
                    Reading::Exact(Some(ScalarValue::String(self.identity.to_owned())))
                }
                Some(FieldDomain::Boolean) => Reading::AnyBoolean,
                Some(FieldDomain::Integer { min, max }) => Reading::Range(*min, *max),
                None => Reading::Exact(None),
            },
            ValueRef::Fact { fact_key } => Reading::Exact(
                self.facts
                    .get(fact_key.as_str())
                    .and_then(|declared| self.snapshot.resolve(declared, self.now_unix_ms).ok())
                    .cloned(),
            ),
            ValueRef::KeyedFact {
                fact_key,
                key_field,
                value_column,
                row_filter,
                absent,
            } => {
                if self.field_domains.get(key_field) != Some(&FieldDomain::Identity) {
                    return Reading::Exact(None);
                }
                let value = self
                    .lists
                    .get(&(fact_key.as_str(), row_filter.as_ref()))
                    .and_then(|rows| match rows.get(self.identity) {
                        Some(row) => row.get(value_column).cloned(),
                        None => Some(absent.clone()),
                    });
                Reading::Exact(value)
            }
        }
    }

    /// An integer comparison: `exact` decides one value, and `range` says whether some value
    /// of a range makes it true and whether some value makes it false.
    fn integer_possible(
        &self,
        value: &ValueRef,
        exact: impl Fn(i64) -> bool,
        range: impl Fn(i64, i64) -> (bool, bool),
    ) -> Possible {
        match self.read(value) {
            Reading::Exact(Some(ScalarValue::Integer(value))) => Possible::exact(exact(value)),
            Reading::Range(min, max) => {
                let (truth, falsity) = range(min, max);
                Possible {
                    truth,
                    falsity,
                    unknown: false,
                }
            }
            Reading::Exact(_) | Reading::AnyBoolean => Possible::UNKNOWN,
        }
    }

    fn possible(&self, predicate: &Predicate) -> Possible {
        match predicate {
            Predicate::IntegerAtLeast { value, threshold } => self.integer_possible(
                value,
                |value| value >= *threshold,
                |min, max| (max >= *threshold, min < *threshold),
            ),
            Predicate::IntegerAtMost { value, threshold } => self.integer_possible(
                value,
                |value| value <= *threshold,
                |min, max| (min <= *threshold, max > *threshold),
            ),
            Predicate::IntegerEquals { value, expected } => self.integer_possible(
                value,
                |value| value == *expected,
                |min, max| {
                    (
                        (min..=max).contains(expected),
                        min != *expected || max != *expected,
                    )
                },
            ),
            Predicate::BooleanEquals { value, expected } => match self.read(value) {
                Reading::Exact(Some(ScalarValue::Boolean(value))) => {
                    Possible::exact(value == *expected)
                }
                Reading::AnyBoolean => Possible {
                    truth: true,
                    falsity: true,
                    unknown: false,
                },
                Reading::Exact(_) | Reading::Range(..) => Possible::UNKNOWN,
            },
            Predicate::StringIn { value, allowed } => match self.read(value) {
                Reading::Exact(Some(ScalarValue::String(value))) => {
                    Possible::exact(allowed.contains(&value))
                }
                _ => Possible::UNKNOWN,
            },
            Predicate::All { of } => {
                let members: Vec<Possible> = of.iter().map(|inner| self.possible(inner)).collect();
                Possible {
                    truth: members.iter().all(|member| member.truth),
                    falsity: members.iter().any(|member| member.falsity),
                    unknown: members.iter().all(|member| member.truth || member.unknown)
                        && members.iter().any(|member| member.unknown),
                }
            }
            Predicate::Any { of } => {
                let members: Vec<Possible> = of.iter().map(|inner| self.possible(inner)).collect();
                Possible {
                    truth: members.iter().any(|member| member.truth),
                    falsity: members.iter().all(|member| member.falsity),
                    unknown: members
                        .iter()
                        .all(|member| member.falsity || member.unknown)
                        && members.iter().any(|member| member.unknown),
                }
            }
            Predicate::Not { of } => {
                let inner = self.possible(of);
                Possible {
                    truth: inner.falsity,
                    falsity: inner.truth,
                    unknown: inner.unknown,
                }
            }
        }
    }

    /// Every transform output the term can take; empty when the candidate is dropped, and
    /// `Err` when an exact unknown input ends the evaluation.
    fn outputs(&self, term: &ScoringTerm) -> Result<Vec<i64>, ()> {
        let (mut outputs, unknown, ranged) = match self.read(&term.value) {
            Reading::Exact(Some(value)) => match transform(&term.transform, &value) {
                Ok(output) => (vec![output], false, false),
                Err(_) => (Vec::new(), true, false),
            },
            Reading::Exact(None) => (Vec::new(), true, false),
            Reading::Range(min, max) => {
                let (outputs, miss) = ranged_outputs(&term.transform, Some((min, max)));
                (outputs, miss, true)
            }
            Reading::AnyBoolean => {
                let (outputs, miss) = ranged_outputs(&term.transform, None);
                (outputs, miss, true)
            }
        };
        match term.on_unknown {
            // A ranged field may also be read as unknown, so its substitution joins too.
            TermUnknownHandling::SubstituteMilli { value_milli } if unknown || ranged => {
                outputs.push(value_milli);
            }
            TermUnknownHandling::AbortEvaluation if unknown => return Err(()),
            _ => {}
        }
        Ok(outputs)
    }
}

/// The outputs a transform takes over an integer range (`Some((min, max))`) or over both
/// boolean values (`None`), and whether some value of the domain misses a lookup that has no
/// default.
fn ranged_outputs(transform: &Transform, range: Option<(i64, i64)>) -> (Vec<i64>, bool) {
    match (transform, range) {
        (Transform::Identity, Some((min, max))) => (vec![min, max], false),
        (
            Transform::Threshold {
                at_least,
                then_milli,
                otherwise_milli,
            },
            Some((min, max)),
        ) => {
            let mut outputs = Vec::new();
            if max >= *at_least {
                outputs.push(*then_milli);
            }
            if min < *at_least {
                outputs.push(*otherwise_milli);
            }
            (outputs, false)
        }
        (
            Transform::Lookup {
                entries,
                default_milli,
            },
            range,
        ) => {
            let reached: Vec<i64> = entries
                .iter()
                .filter(|entry| match (&entry.key, range) {
                    (LookupKey::Integer(key), Some((min, max))) => (min..=max).contains(key),
                    (LookupKey::Boolean(_), None) => true,
                    _ => false,
                })
                .map(|entry| entry.value_milli)
                .collect();
            let size = match range {
                Some((min, max)) => i128::from(max) - i128::from(min) + 1,
                None => 2,
            };
            let mut outputs = reached;
            let covered = outputs.len() as i128 >= size;
            match (covered, default_milli) {
                (true, _) => (outputs, false),
                (false, Some(default)) => {
                    outputs.push(*default);
                    (outputs, false)
                }
                (false, None) => (outputs, true),
            }
        }
        // Validation admits no other transform over a ranged field.
        (Transform::Identity | Transform::Threshold { .. }, None) => (Vec::new(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldDeclaration, HardGate};
    use actingcommand_contract::{FactContent, FactRecord, FactScalar, FactScope, FactValue};

    const NOW: u64 = 1_000_500;

    fn members() -> Vec<String> {
        ["member-a", "member-b", "member-c"]
            .map(str::to_owned)
            .to_vec()
    }

    /// The list `session.example.list.targets` with one `rank` per listed member.
    fn list_facts(policy: &SelectionPolicy, ranks: &[(&str, i64)]) -> SelectionFactSnapshot {
        let rows = ranks
            .iter()
            .map(|(id, rank)| {
                BTreeMap::from([
                    ("id".to_owned(), FactScalar::String((*id).to_owned())),
                    ("rank".to_owned(), FactScalar::Integer(*rank)),
                ])
            })
            .collect();
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: "instance-a".to_owned(),
            },
            key: "session.example.list.targets".to_owned(),
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
        };
        SelectionFactSnapshot::from_fact_records_with_lists(
            "snapshot-list",
            NOW,
            &[record],
            &policy.record_list_keys(),
        )
    }

    fn domains(extra: &[(&str, FieldDomain)]) -> BTreeMap<String, FieldDomain> {
        let mut domains = BTreeMap::from([("member".to_owned(), FieldDomain::Identity)]);
        for (name, domain) in extra {
            domains.insert((*name).to_owned(), *domain);
        }
        domains
    }

    /// The keyed document plus one integer field `name` scored by `transform`.
    fn with_integer_term(
        name: &str,
        transform: Transform,
        weight_milli: i64,
        on_unknown: TermUnknownHandling,
    ) -> SelectionPolicy {
        let mut policy = crate::schema::keyed_policy();
        policy.fields.push(FieldDeclaration {
            name: name.to_owned(),
            value_type: ValueType::Integer,
        });
        policy.scoring.push(ScoringTerm {
            term_id: format!("term-{name}"),
            value: ValueRef::Field {
                field: name.to_owned(),
            },
            transform,
            weight_milli,
            on_unknown,
        });
        policy
    }

    #[test]
    fn a_ranked_list_bounds_each_identity_by_its_own_row() {
        let policy = crate::schema::keyed_policy();
        let facts = list_facts(&policy, &[("member-a", 2), ("member-c", 3)]);
        let bound = |identities: &[String]| {
            upper_bound(&policy, &facts, NOW, identities, &domains(&[])).expect("bound")
        };
        // An unknown instance without a known identity is bounded over the whole domain.
        assert_eq!(bound(&members()), UpperBound::Max(3));
        // One with a known identity is bounded at that identity only.
        assert_eq!(bound(&["member-a".to_owned()]), UpperBound::Max(2));
        // An unlisted identity reads `absent`, and the listed gate is false for it.
        assert_eq!(bound(&["member-b".to_owned()]), UpperBound::NoneEligible);
        assert_eq!(bound(&[]), UpperBound::NoneEligible);
    }

    #[test]
    fn a_member_set_bounds_every_listed_identity_alike() {
        let policy = crate::schema::keyed_policy();
        let facts = list_facts(&policy, &[("member-a", 1), ("member-c", 1)]);
        for identities in [members(), vec!["member-a".to_owned()]] {
            assert_eq!(
                upper_bound(&policy, &facts, NOW, &identities, &domains(&[])).expect("bound"),
                UpperBound::Max(1)
            );
        }
    }

    #[test]
    fn ranged_fields_take_every_value_of_their_domain() {
        // An `ocr_integer` field in 1..=90 adds its largest value through `identity`.
        let level = with_integer_term(
            "level",
            Transform::Identity,
            1_000,
            TermUnknownHandling::DropCandidate,
        );
        let facts = list_facts(&level, &[("member-a", 2), ("member-c", 3)]);
        let level_domain = domains(&[("level", FieldDomain::Integer { min: 1, max: 90 })]);
        assert_eq!(
            upper_bound(&level, &facts, NOW, &members(), &level_domain).expect("bound"),
            UpperBound::Max(93)
        );
        // A gate no value of the domain passes excludes every identity.
        let mut gated = level.clone();
        gated.gates.push(HardGate {
            gate_id: "gate-level".to_owned(),
            predicate: Predicate::IntegerAtLeast {
                value: ValueRef::Field {
                    field: "level".to_owned(),
                },
                threshold: 95,
            },
            on_unknown: crate::schema::GateUnknownHandling::DropCandidate,
        });
        assert_eq!(
            upper_bound(&gated, &facts, NOW, &members(), &level_domain).expect("bound"),
            UpperBound::NoneEligible
        );

        // A `measure_milli` field in 0..=1000 through a threshold, under a negative weight:
        // the lower output gives the larger contribution.
        let threshold = Transform::Threshold {
            at_least: 500,
            then_milli: 2_000,
            otherwise_milli: -1_000,
        };
        let measure = with_integer_term(
            "quality",
            threshold.clone(),
            -500,
            TermUnknownHandling::DropCandidate,
        );
        let quality_domain = domains(&[("quality", FieldDomain::Integer { min: 0, max: 1_000 })]);
        assert_eq!(
            upper_bound(&measure, &facts, NOW, &members(), &quality_domain).expect("bound"),
            UpperBound::Max(503)
        );

        // A declared substitution joins the candidates for the maximum.
        let substituted = with_integer_term(
            "quality",
            threshold,
            1_000,
            TermUnknownHandling::SubstituteMilli { value_milli: 4_000 },
        );
        assert_eq!(
            upper_bound(&substituted, &facts, NOW, &members(), &quality_domain).expect("bound"),
            UpperBound::Max(4_003)
        );
    }

    #[test]
    fn an_overflow_or_an_unreadable_list_is_unbounded() {
        const SAFE_MAX: i64 = 9_007_199_254_740_991;
        let policy = with_integer_term(
            "level",
            Transform::Identity,
            SAFE_MAX,
            TermUnknownHandling::DropCandidate,
        );
        let facts = list_facts(&policy, &[("member-a", 2)]);
        let wide = domains(&[(
            "level",
            FieldDomain::Integer {
                min: 0,
                max: SAFE_MAX,
            },
        )]);
        assert_eq!(
            upper_bound(&policy, &facts, NOW, &members(), &wide).expect("bound"),
            UpperBound::Unbounded
        );

        let keyed = crate::schema::keyed_policy();
        let empty = SelectionFactSnapshot::from_fact_records_with_lists(
            "snapshot-list",
            NOW,
            &[],
            &keyed.record_list_keys(),
        );
        assert_eq!(
            upper_bound(&keyed, &empty, NOW, &members(), &domains(&[])).expect("bound"),
            UpperBound::Unbounded
        );
    }
}
