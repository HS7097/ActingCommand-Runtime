// SPDX-License-Identifier: AGPL-3.0-only

//! The selection-policy document: the declared half of one selection decision.
//!
//! A document names the candidate layout it applies to, declares every candidate field and
//! every instance fact it is allowed to read, then states the hard gates, the weighted
//! scoring terms, how many candidates are required, and how ties are broken. Nothing in the
//! document is optional-by-omission: a term or gate that can meet an unknown input must say
//! what happens then.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// Schema version accepted by this crate.
pub const SELECTION_POLICY_SCHEMA_VERSION: &str = "actingcommand.selection-policy.v1";

/// Upper bound on one evaluated candidate set.
pub const MAX_CANDIDATES: usize = 4_096;
/// Upper bound on declared candidate fields.
pub const MAX_FIELDS: usize = 128;
/// Upper bound on declared fact references.
pub const MAX_FACTS: usize = 128;
/// Upper bound on hard gates.
pub const MAX_GATES: usize = 128;
/// Upper bound on scoring terms.
pub const MAX_SCORING_TERMS: usize = 512;
/// Upper bound on tie-break keys.
pub const MAX_TIE_BREAK_KEYS: usize = 16;
/// Upper bound on entries in one lookup transform.
pub const MAX_LOOKUP_ENTRIES: usize = 512;
/// Upper bound on nested predicate depth, matching the scheduling predicate limit.
pub const MAX_PREDICATE_DEPTH: usize = 16;
/// Upper bound on predicate nodes in one gate, matching the scheduling predicate limit.
pub const MAX_PREDICATE_NODES: usize = 512;
/// Upper bound on one identifier, fact key, or outcome key.
pub const MAX_ID_BYTES: usize = 128;
/// Upper bound on the members of one enumerated string type.
pub const MAX_ENUM_VALUES: usize = 128;

/// Closed set of failure codes reported by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionErrorCode {
    ArithmeticOverflow,
    DanglingReference,
    DocumentTooLarge,
    DuplicateId,
    DuplicateKey,
    FloatRejected,
    IntegerOutOfRange,
    InvalidArguments,
    InvalidJson,
    LimitExceeded,
    MissingRequiredField,
    ReadFailed,
    TypeMismatch,
    UnsupportedSchemaVersion,
}

/// One typed failure with a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionError {
    pub code: SelectionErrorCode,
    pub detail: String,
}

impl SelectionError {
    pub fn new(code: SelectionErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> SelectionErrorCode {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for SelectionError {}

/// Closed value model shared by candidate fields, facts, and lookup keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueType {
    Integer,
    Boolean,
    EnumString { allowed: Vec<String> },
}

/// The candidate layout and the resource-declared outcome keys this document may emit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliesTo {
    pub candidate_layout_id: String,
    pub outcome_keys: OutcomeKeys,
}

/// One resource-declared string per decision outcome; Runtime never invents these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeKeys {
    pub selected: String,
    pub empty: String,
    pub insufficient: String,
    pub ambiguous: String,
    pub unknown: String,
}

impl OutcomeKeys {
    pub(crate) fn each(&self) -> [&str; 5] {
        [
            &self.selected,
            &self.empty,
            &self.insufficient,
            &self.ambiguous,
            &self.unknown,
        ]
    }
}

/// One candidate field the document is allowed to read, with its declared type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDeclaration {
    pub name: String,
    pub value_type: ValueType,
}

/// One instance fact the document is allowed to read, with its declared type and freshness.
///
/// `max_age_ms` is the document's own freshness bound. A fact published longer ago than this
/// is unknown to this document even when its own TTL has not run out yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactDeclaration {
    pub fact_key: String,
    pub value_type: ValueType,
    pub max_age_ms: u64,
    pub minimum_confidence_milli: u16,
}

/// Where one term or predicate reads its value from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueRef {
    Field { field: String },
    Fact { fact_key: String },
}

/// One lookup key paired with the integer milli value it maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupEntry {
    pub key: LookupKey,
    pub value_milli: i64,
}

/// Closed key model for lookup transforms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LookupKey {
    Integer(i64),
    Boolean(bool),
    String(String),
}

/// How one resolved value becomes an integer milli term input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transform {
    /// Passes an integer value through unchanged.
    Identity,
    /// Maps an integer value to one of two declared milli values.
    Threshold {
        at_least: i64,
        then_milli: i64,
        otherwise_milli: i64,
    },
    /// Maps a value to a declared milli value through an integer table.
    Lookup {
        entries: Vec<LookupEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_milli: Option<i64>,
    },
}

/// What a scoring term does when its input is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TermUnknownHandling {
    /// Removes the candidate from ranking and records the unknown reason.
    DropCandidate,
    /// Ends the whole evaluation with an unknown outcome.
    AbortEvaluation,
    /// Uses a value the document states in the open, recorded as a substitution.
    SubstituteMilli { value_milli: i64 },
}

/// What a hard gate does when its predicate cannot be decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GateUnknownHandling {
    /// Removes the candidate from ranking and records the unknown reason.
    DropCandidate,
    /// Ends the whole evaluation with an unknown outcome.
    AbortEvaluation,
    /// Uses a verdict the document states in the open, recorded as a substitution.
    SubstituteVerdict { passes: bool },
}

/// Three-valued predicate vocabulary over declared fields and facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Predicate {
    IntegerAtLeast {
        value: ValueRef,
        threshold: i64,
    },
    IntegerAtMost {
        value: ValueRef,
        threshold: i64,
    },
    IntegerEquals {
        value: ValueRef,
        expected: i64,
    },
    BooleanEquals {
        value: ValueRef,
        expected: bool,
    },
    StringIn {
        value: ValueRef,
        allowed: Vec<String>,
    },
    All {
        of: Vec<Predicate>,
    },
    Any {
        of: Vec<Predicate>,
    },
    Not {
        of: Box<Predicate>,
    },
}

/// One hard gate: a candidate whose predicate does not hold is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardGate {
    pub gate_id: String,
    pub predicate: Predicate,
    pub on_unknown: GateUnknownHandling,
}

/// One weighted scoring term.
///
/// The contribution is `transform_output * weight_milli / 1000`, truncated toward zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringTerm {
    pub term_id: String,
    pub value: ValueRef,
    pub transform: Transform,
    pub weight_milli: i64,
    pub on_unknown: TermUnknownHandling,
}

/// How many candidates the document requires and what an empty answer means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    /// Requires exactly `required_count` survivors; fewer is an insufficient outcome.
    TopK,
    /// Requires exactly one survivor; `required_count` must be one.
    ExactlyOne,
    /// Takes up to `required_count` survivors and accepts an empty answer.
    NoneAllowed,
}

/// The selection mode paired with its required count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRequirement {
    pub mode: SelectionMode,
    pub required_count: u32,
}

/// Sort direction for one tie-break key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    HighestFirst,
    LowestFirst,
}

/// One ordered tie-break key applied after the score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TieBreakKey {
    Value {
        value: ValueRef,
        direction: SortDirection,
    },
    CandidateId {
        direction: SortDirection,
    },
}

/// One selection-policy document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    pub schema_version: String,
    pub policy_id: String,
    pub applies_to: AppliesTo,
    pub fields: Vec<FieldDeclaration>,
    pub facts: Vec<FactDeclaration>,
    pub gates: Vec<HardGate>,
    pub scoring: Vec<ScoringTerm>,
    pub selection: SelectionRequirement,
    pub tie_break: Vec<TieBreakKey>,
}

impl SelectionPolicy {
    /// Checks the document against the schema version, the declared limits, and the rule that
    /// every referenced field and fact is declared with a type the transform can consume.
    pub fn validate(&self) -> Result<(), SelectionError> {
        if self.schema_version != SELECTION_POLICY_SCHEMA_VERSION {
            return Err(SelectionError::new(
                SelectionErrorCode::UnsupportedSchemaVersion,
                format!(
                    "`{}` is not `{SELECTION_POLICY_SCHEMA_VERSION}`",
                    self.schema_version
                ),
            ));
        }
        check_id(&self.policy_id, "policy_id")?;
        check_id(&self.applies_to.candidate_layout_id, "candidate_layout_id")?;
        for key in self.applies_to.outcome_keys.each() {
            check_id(key, "outcome_key")?;
        }
        check_limit(self.fields.len(), MAX_FIELDS, "fields")?;
        check_limit(self.facts.len(), MAX_FACTS, "facts")?;
        check_limit(self.gates.len(), MAX_GATES, "gates")?;
        check_limit(self.scoring.len(), MAX_SCORING_TERMS, "scoring")?;
        check_limit(self.tie_break.len(), MAX_TIE_BREAK_KEYS, "tie_break")?;

        let mut fields = BTreeMap::new();
        for declaration in &self.fields {
            check_id(&declaration.name, "field name")?;
            check_value_type(&declaration.value_type, &declaration.name)?;
            if fields
                .insert(declaration.name.as_str(), &declaration.value_type)
                .is_some()
            {
                return Err(SelectionError::new(
                    SelectionErrorCode::DuplicateId,
                    format!("field `{}` is declared twice", declaration.name),
                ));
            }
        }
        let mut facts = BTreeMap::new();
        for declaration in &self.facts {
            check_id(&declaration.fact_key, "fact_key")?;
            check_value_type(&declaration.value_type, &declaration.fact_key)?;
            if declaration.max_age_ms == 0 {
                return Err(SelectionError::new(
                    SelectionErrorCode::MissingRequiredField,
                    format!("fact `{}` declares a zero max_age_ms", declaration.fact_key),
                ));
            }
            if declaration.minimum_confidence_milli > 1_000 {
                return Err(SelectionError::new(
                    SelectionErrorCode::IntegerOutOfRange,
                    format!(
                        "fact `{}` declares a confidence above one thousand milli",
                        declaration.fact_key
                    ),
                ));
            }
            if facts
                .insert(declaration.fact_key.as_str(), &declaration.value_type)
                .is_some()
            {
                return Err(SelectionError::new(
                    SelectionErrorCode::DuplicateId,
                    format!("fact `{}` is declared twice", declaration.fact_key),
                ));
            }
        }
        let scope = Scope { fields, facts };

        let mut gate_ids = BTreeSet::new();
        for gate in &self.gates {
            check_id(&gate.gate_id, "gate_id")?;
            if !gate_ids.insert(gate.gate_id.as_str()) {
                return Err(SelectionError::new(
                    SelectionErrorCode::DuplicateId,
                    format!("gate `{}` is declared twice", gate.gate_id),
                ));
            }
            let mut nodes = 0usize;
            check_predicate(&gate.predicate, &scope, 1, &mut nodes, &gate.gate_id)?;
        }

        let mut term_ids = BTreeSet::new();
        for term in &self.scoring {
            check_id(&term.term_id, "term_id")?;
            if !term_ids.insert(term.term_id.as_str()) {
                return Err(SelectionError::new(
                    SelectionErrorCode::DuplicateId,
                    format!("term `{}` is declared twice", term.term_id),
                ));
            }
            let value_type = scope.resolve(&term.value, &term.term_id)?;
            check_transform(&term.transform, value_type, &term.term_id)?;
        }

        for key in &self.tie_break {
            if let TieBreakKey::Value { value, .. } = key {
                scope.resolve(value, "tie_break")?;
            }
        }

        match self.selection.mode {
            SelectionMode::TopK => {
                if self.selection.required_count == 0 {
                    return Err(SelectionError::new(
                        SelectionErrorCode::MissingRequiredField,
                        "top_k requires a required_count of at least one".to_owned(),
                    ));
                }
            }
            SelectionMode::ExactlyOne => {
                if self.selection.required_count != 1 {
                    return Err(SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        "exactly_one requires a required_count of one".to_owned(),
                    ));
                }
            }
            SelectionMode::NoneAllowed => {}
        }
        if self.selection.required_count as usize > MAX_CANDIDATES {
            return Err(SelectionError::new(
                SelectionErrorCode::LimitExceeded,
                format!("required_count exceeds the {MAX_CANDIDATES} candidate limit"),
            ));
        }
        Ok(())
    }
}

pub(crate) struct Scope<'a> {
    pub(crate) fields: BTreeMap<&'a str, &'a ValueType>,
    pub(crate) facts: BTreeMap<&'a str, &'a ValueType>,
}

impl<'a> Scope<'a> {
    pub(crate) fn resolve(
        &self,
        reference: &ValueRef,
        owner: &str,
    ) -> Result<&'a ValueType, SelectionError> {
        match reference {
            ValueRef::Field { field } => {
                self.fields.get(field.as_str()).copied().ok_or_else(|| {
                    SelectionError::new(
                        SelectionErrorCode::DanglingReference,
                        format!("`{owner}` reads undeclared field `{field}`"),
                    )
                })
            }
            ValueRef::Fact { fact_key } => {
                self.facts.get(fact_key.as_str()).copied().ok_or_else(|| {
                    SelectionError::new(
                        SelectionErrorCode::DanglingReference,
                        format!("`{owner}` reads undeclared fact `{fact_key}`"),
                    )
                })
            }
        }
    }
}

fn check_id(value: &str, label: &str) -> Result<(), SelectionError> {
    if value.is_empty() {
        return Err(SelectionError::new(
            SelectionErrorCode::MissingRequiredField,
            format!("{label} is empty"),
        ));
    }
    if value.len() > MAX_ID_BYTES {
        return Err(SelectionError::new(
            SelectionErrorCode::LimitExceeded,
            format!("{label} exceeds {MAX_ID_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn check_limit(actual: usize, limit: usize, label: &str) -> Result<(), SelectionError> {
    if actual > limit {
        return Err(SelectionError::new(
            SelectionErrorCode::LimitExceeded,
            format!("{label} holds {actual} entries above the {limit} limit"),
        ));
    }
    Ok(())
}

fn check_value_type(value_type: &ValueType, owner: &str) -> Result<(), SelectionError> {
    if let ValueType::EnumString { allowed } = value_type {
        check_limit(allowed.len(), MAX_ENUM_VALUES, owner)?;
        if allowed.is_empty() {
            return Err(SelectionError::new(
                SelectionErrorCode::MissingRequiredField,
                format!("`{owner}` declares an empty enum_string"),
            ));
        }
        let mut seen = BTreeSet::new();
        for member in allowed {
            check_id(member, "enum_string member")?;
            if !seen.insert(member.as_str()) {
                return Err(SelectionError::new(
                    SelectionErrorCode::DuplicateId,
                    format!("`{owner}` repeats enum_string member `{member}`"),
                ));
            }
        }
    }
    Ok(())
}

fn check_predicate(
    predicate: &Predicate,
    scope: &Scope<'_>,
    depth: usize,
    nodes: &mut usize,
    owner: &str,
) -> Result<(), SelectionError> {
    *nodes += 1;
    if depth > MAX_PREDICATE_DEPTH {
        return Err(SelectionError::new(
            SelectionErrorCode::LimitExceeded,
            format!("gate `{owner}` nests deeper than {MAX_PREDICATE_DEPTH}"),
        ));
    }
    if *nodes > MAX_PREDICATE_NODES {
        return Err(SelectionError::new(
            SelectionErrorCode::LimitExceeded,
            format!("gate `{owner}` holds more than {MAX_PREDICATE_NODES} nodes"),
        ));
    }
    let expect = |value: &ValueRef, wanted: &str| -> Result<(), SelectionError> {
        let value_type = scope.resolve(value, owner)?;
        let matches = matches!(
            (wanted, value_type),
            ("integer", ValueType::Integer)
                | ("boolean", ValueType::Boolean)
                | ("string", ValueType::EnumString { .. })
        );
        if matches {
            Ok(())
        } else {
            Err(SelectionError::new(
                SelectionErrorCode::TypeMismatch,
                format!("gate `{owner}` compares a non-{wanted} value"),
            ))
        }
    };
    match predicate {
        Predicate::IntegerAtLeast { value, .. }
        | Predicate::IntegerAtMost { value, .. }
        | Predicate::IntegerEquals { value, .. } => expect(value, "integer")?,
        Predicate::BooleanEquals { value, .. } => expect(value, "boolean")?,
        Predicate::StringIn { value, allowed } => {
            expect(value, "string")?;
            check_limit(allowed.len(), MAX_ENUM_VALUES, owner)?;
        }
        Predicate::All { of } | Predicate::Any { of } => {
            if of.is_empty() {
                return Err(SelectionError::new(
                    SelectionErrorCode::MissingRequiredField,
                    format!("gate `{owner}` holds an empty predicate list"),
                ));
            }
            for inner in of {
                check_predicate(inner, scope, depth + 1, nodes, owner)?;
            }
        }
        Predicate::Not { of } => check_predicate(of, scope, depth + 1, nodes, owner)?,
    }
    Ok(())
}

fn check_transform(
    transform: &Transform,
    value_type: &ValueType,
    owner: &str,
) -> Result<(), SelectionError> {
    match transform {
        Transform::Identity | Transform::Threshold { .. } => {
            if !matches!(value_type, ValueType::Integer) {
                return Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("term `{owner}` needs a lookup for a non-integer value"),
                ));
            }
        }
        Transform::Lookup { entries, .. } => {
            check_limit(entries.len(), MAX_LOOKUP_ENTRIES, owner)?;
            if entries.is_empty() {
                return Err(SelectionError::new(
                    SelectionErrorCode::MissingRequiredField,
                    format!("term `{owner}` declares an empty lookup"),
                ));
            }
            let mut seen = BTreeSet::new();
            for entry in entries {
                if !seen.insert(&entry.key) {
                    return Err(SelectionError::new(
                        SelectionErrorCode::DuplicateId,
                        format!("term `{owner}` repeats a lookup key"),
                    ));
                }
                let matches = match (&entry.key, value_type) {
                    (LookupKey::Integer(_), ValueType::Integer) => true,
                    (LookupKey::Boolean(_), ValueType::Boolean) => true,
                    (LookupKey::String(member), ValueType::EnumString { allowed }) => {
                        allowed.iter().any(|value| value == member)
                    }
                    _ => false,
                };
                if !matches {
                    return Err(SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        format!("term `{owner}` declares a lookup key outside the value type"),
                    ));
                }
            }
        }
    }
    Ok(())
}
