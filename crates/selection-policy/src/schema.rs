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

use crate::facts::ScalarValue;

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
/// Upper bound on the members of one enumerated string type. It equals the largest business
/// identity domain a pack may declare, because an identity field's enum must equal its domain.
pub const MAX_ENUM_VALUES: usize = 1_024;
/// Upper bound on the declared value columns of one record list: the fact contract's 64 fields
/// per row, less the key column.
pub const MAX_RECORD_LIST_COLUMNS: usize = 63;

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
    EnumString {
        allowed: Vec<String>,
    },
    /// A record-list fact, read one row at a time through [`ValueRef::KeyedFact`]. Legal only
    /// in `facts[]`: each row holds the string `key_column` and every declared column with its
    /// scalar type; columns a document does not declare are not read.
    RecordList {
        key_column: String,
        columns: BTreeMap<String, ValueType>,
    },
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
    Field {
        field: String,
    },
    Fact {
        fact_key: String,
    },
    /// The `value_column` of the row of the record list `fact_key` whose key column equals the
    /// candidate's `key_field`, among the rows `where` selects. With no such row the value is
    /// `absent`, a known value of the column's type.
    KeyedFact {
        fact_key: String,
        key_field: String,
        value_column: String,
        #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
        row_filter: Option<RowFilter>,
        absent: ScalarValue,
    },
}

/// Selects the rows of one kind: those whose enumerated `column` equals `equals`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowFilter {
    pub column: String,
    pub equals: String,
}

impl ValueRef {
    pub(crate) fn is_keyed_fact(&self) -> bool {
        matches!(self, Self::KeyedFact { .. })
    }
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

impl Predicate {
    /// Every value the predicate reads, in document order.
    pub(crate) fn collect_values<'a>(&'a self, values: &mut Vec<&'a ValueRef>) {
        match self {
            Self::IntegerAtLeast { value, .. }
            | Self::IntegerAtMost { value, .. }
            | Self::IntegerEquals { value, .. }
            | Self::BooleanEquals { value, .. }
            | Self::StringIn { value, .. } => values.push(value),
            Self::All { of } | Self::Any { of } => {
                for inner in of {
                    inner.collect_values(values);
                }
            }
            Self::Not { of } => of.collect_values(values),
        }
    }

    fn reads_keyed_fact(&self) -> bool {
        let mut values = Vec::new();
        self.collect_values(&mut values);
        values.into_iter().any(ValueRef::is_keyed_fact)
    }
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
            if let ValueType::RecordList { .. } = declaration.value_type {
                return Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("field={} value_type=record_list", declaration.name),
                ));
            }
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
            // A list that cannot be read ends the evaluation; a rule never stands in for it.
            if gate.predicate.reads_keyed_fact()
                && gate.on_unknown != GateUnknownHandling::AbortEvaluation
            {
                return Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("gate={} reads=keyed_fact on_unknown", gate.gate_id),
                ));
            }
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
            if term.value.is_keyed_fact() && term.on_unknown != TermUnknownHandling::AbortEvaluation
            {
                return Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("term={} reads=keyed_fact on_unknown", term.term_id),
                ));
            }
        }

        for key in &self.tie_break {
            if let TieBreakKey::Value { value, .. } = key {
                // An enumerated string has no declared order, only a declared member list, so
                // ordering one would fall back to its bytes. Score it through a lookup instead.
                if let ValueType::EnumString { .. } = scope.resolve(value, "tie_break")? {
                    return Err(SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        "a tie-break key cannot order an enum_string value".to_owned(),
                    ));
                }
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

    /// The fact keys this document declares as record lists, which only `keyed_fact` reads.
    pub fn record_list_keys(&self) -> BTreeSet<&str> {
        self.facts
            .iter()
            .filter(|declaration| matches!(declaration.value_type, ValueType::RecordList { .. }))
            .map(|declaration| declaration.fact_key.as_str())
            .collect()
    }

    /// Whether this document declares a record list. Only an in-task consumer reads one; the
    /// scheduling consumer refuses such a document.
    pub fn uses_record_lists(&self) -> bool {
        !self.record_list_keys().is_empty()
    }

    /// Every value the gates, the terms and the tie-break keys read, in document order.
    pub(crate) fn value_refs(&self) -> Vec<&ValueRef> {
        let mut values = Vec::new();
        for gate in &self.gates {
            gate.predicate.collect_values(&mut values);
        }
        values.extend(self.scoring.iter().map(|term| &term.value));
        values.extend(self.tie_break.iter().filter_map(|key| match key {
            TieBreakKey::Value { value, .. } => Some(value),
            TieBreakKey::CandidateId { .. } => None,
        }));
        values
    }
}

pub(crate) struct Scope<'a> {
    pub(crate) fields: BTreeMap<&'a str, &'a ValueType>,
    pub(crate) facts: BTreeMap<&'a str, &'a ValueType>,
}

impl<'a> Scope<'a> {
    /// The declared type of the value `reference` reads. A record list is read only through
    /// `keyed_fact`, whose value has the type of its `value_column`.
    pub(crate) fn resolve(
        &self,
        reference: &ValueRef,
        owner: &str,
    ) -> Result<&'a ValueType, SelectionError> {
        match reference {
            ValueRef::Field { field } => self.field(field, owner),
            ValueRef::Fact { fact_key } => match self.fact(fact_key, owner)? {
                ValueType::RecordList { .. } => Err(SelectionError::new(
                    SelectionErrorCode::TypeMismatch,
                    format!("owner={owner} fact={fact_key} value_type=record_list"),
                )),
                value_type => Ok(value_type),
            },
            ValueRef::KeyedFact {
                fact_key,
                key_field,
                value_column,
                row_filter,
                absent,
            } => {
                let mismatch = |member: &str| {
                    SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        format!("owner={owner} fact={fact_key} keyed_fact={member}"),
                    )
                };
                let dangling = |member: &str| {
                    SelectionError::new(
                        SelectionErrorCode::DanglingReference,
                        format!("owner={owner} fact={fact_key} keyed_fact={member}"),
                    )
                };
                let ValueType::RecordList { columns, .. } = self.fact(fact_key, owner)? else {
                    return Err(mismatch("fact_key"));
                };
                if !matches!(self.field(key_field, owner)?, ValueType::EnumString { .. }) {
                    return Err(mismatch("key_field"));
                }
                let column = columns
                    .get(value_column)
                    .ok_or_else(|| dangling("value_column"))?;
                if let Some(filter) = row_filter {
                    match columns.get(&filter.column) {
                        None => return Err(dangling("where")),
                        Some(ValueType::EnumString { allowed })
                            if allowed.iter().any(|member| member == &filter.equals) => {}
                        Some(_) => return Err(mismatch("where")),
                    }
                }
                if !absent.matches(column) {
                    return Err(mismatch("absent"));
                }
                Ok(column)
            }
        }
    }

    fn field(&self, field: &str, owner: &str) -> Result<&'a ValueType, SelectionError> {
        self.fields.get(field).copied().ok_or_else(|| {
            SelectionError::new(
                SelectionErrorCode::DanglingReference,
                format!("`{owner}` reads undeclared field `{field}`"),
            )
        })
    }

    fn fact(&self, fact_key: &str, owner: &str) -> Result<&'a ValueType, SelectionError> {
        self.facts.get(fact_key).copied().ok_or_else(|| {
            SelectionError::new(
                SelectionErrorCode::DanglingReference,
                format!("`{owner}` reads undeclared fact `{fact_key}`"),
            )
        })
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
    match value_type {
        ValueType::Integer | ValueType::Boolean => {}
        ValueType::EnumString { allowed } => {
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
        ValueType::RecordList {
            key_column,
            columns,
        } => {
            check_id(key_column, "key_column")?;
            check_limit(columns.len(), MAX_RECORD_LIST_COLUMNS, owner)?;
            for (column, column_type) in columns {
                check_id(column, "column name")?;
                if column == key_column {
                    return Err(SelectionError::new(
                        SelectionErrorCode::DuplicateId,
                        format!("fact={owner} column={column} key_column"),
                    ));
                }
                if let ValueType::RecordList { .. } = column_type {
                    return Err(SelectionError::new(
                        SelectionErrorCode::TypeMismatch,
                        format!("fact={owner} column={column} value_type=record_list"),
                    ));
                }
                check_value_type(column_type, owner)?;
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
            if let ValueType::EnumString { allowed: declared } = scope.resolve(value, owner)? {
                for member in allowed {
                    if !declared.iter().any(|value| value == member) {
                        return Err(SelectionError::new(
                            SelectionErrorCode::TypeMismatch,
                            format!("gate `{owner}` lists `{member}` outside the value type"),
                        ));
                    }
                }
            }
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

/// A neutral in-task document that reads one record list through `keyed_fact`: a gate that
/// the member is listed (`rank` at least one, `absent` zero) and a term that scores its rank.
#[cfg(test)]
pub(crate) fn keyed_policy() -> SelectionPolicy {
    let list = serde_json::json!({
        "source": "keyed_fact", "fact_key": "session.example.list.targets",
        "key_field": "member", "value_column": "rank",
        "absent": {"type": "integer", "value": 0}
    });
    serde_json::from_value(serde_json::json!({
        "schema_version": SELECTION_POLICY_SCHEMA_VERSION,
        "policy_id": "policy-list",
        "applies_to": {
            "candidate_layout_id": "layout-list",
            "outcome_keys": {
                "selected": "list-selected", "empty": "list-empty",
                "insufficient": "list-insufficient", "ambiguous": "list-ambiguous",
                "unknown": "list-unknown"
            }
        },
        "fields": [{
            "name": "member",
            "value_type": {"type": "enum_string", "allowed": ["member-a", "member-b", "member-c"]}
        }],
        "facts": [{
            "fact_key": "session.example.list.targets",
            "value_type": {
                "type": "record_list", "key_column": "id",
                "columns": {"rank": {"type": "integer"}}
            },
            "max_age_ms": 3_600_000,
            "minimum_confidence_milli": 1_000
        }],
        "gates": [{
            "gate_id": "gate-listed",
            "predicate": {"kind": "integer_at_least", "value": list.clone(), "threshold": 1},
            "on_unknown": {"kind": "abort_evaluation"}
        }],
        "scoring": [{
            "term_id": "term-rank", "value": list, "transform": {"kind": "identity"},
            "weight_milli": 1_000, "on_unknown": {"kind": "abort_evaluation"}
        }],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("keyed policy decodes")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn fixture() -> SelectionPolicy {
        serde_json::from_str(include_str!("../tests/fixtures/policy.json"))
            .expect("fixture policy decodes")
    }

    /// The `keyed_fact` reference of the keyed document's term.
    fn list_reference(policy: &mut SelectionPolicy) -> &mut ValueRef {
        &mut policy.scoring[0].value
    }

    #[test]
    fn a_keyed_fact_document_validates_only_when_every_reader_is_fail_closed() {
        let policy = keyed_policy();
        policy.validate().expect("keyed policy validates");
        assert_eq!(
            policy.record_list_keys(),
            BTreeSet::from(["session.example.list.targets"])
        );
        assert!(policy.uses_record_lists());
        assert!(!fixture().uses_record_lists());

        // A tie-break key may read a list: the up-front check already guarantees it.
        let mut tie_break = keyed_policy();
        tie_break.tie_break.insert(
            0,
            TieBreakKey::Value {
                value: tie_break.scoring[0].value.clone(),
                direction: SortDirection::HighestFirst,
            },
        );
        tie_break.validate().expect("keyed tie-break validates");

        let cases: [(&str, fn(&mut SelectionPolicy), SelectionErrorCode); 10] = [
            (
                "record_list as a field",
                |policy| policy.fields[0].value_type = policy.facts[0].value_type.clone(),
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "plain fact reference to a list",
                |policy| {
                    policy.scoring[0].value = ValueRef::Fact {
                        fact_key: "session.example.list.targets".to_owned(),
                    }
                },
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "term that drops on an unreadable list",
                |policy| policy.scoring[0].on_unknown = TermUnknownHandling::DropCandidate,
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "gate that substitutes for an unreadable list",
                |policy| {
                    policy.gates[0].on_unknown =
                        GateUnknownHandling::SubstituteVerdict { passes: false }
                },
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "key field of another type",
                |policy| {
                    policy.fields.push(FieldDeclaration {
                        name: "level".to_owned(),
                        value_type: ValueType::Integer,
                    });
                    if let ValueRef::KeyedFact { key_field, .. } = list_reference(policy) {
                        *key_field = "level".to_owned();
                    }
                },
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "undeclared value column",
                |policy| {
                    if let ValueRef::KeyedFact { value_column, .. } = list_reference(policy) {
                        *value_column = "count".to_owned();
                    }
                },
                SelectionErrorCode::DanglingReference,
            ),
            (
                "where over a non-enumerated column",
                |policy| {
                    if let ValueRef::KeyedFact { row_filter, .. } = list_reference(policy) {
                        *row_filter = Some(RowFilter {
                            column: "rank".to_owned(),
                            equals: "kind-x".to_owned(),
                        });
                    }
                },
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "absent of another type",
                |policy| {
                    if let ValueRef::KeyedFact { absent, .. } = list_reference(policy) {
                        *absent = ScalarValue::Boolean(false);
                    }
                },
                SelectionErrorCode::TypeMismatch,
            ),
            (
                "key column among the columns",
                |policy| {
                    if let ValueType::RecordList { columns, .. } = &mut policy.facts[0].value_type {
                        columns.insert(
                            "id".to_owned(),
                            ValueType::EnumString {
                                allowed: vec!["member-a".to_owned()],
                            },
                        );
                    }
                },
                SelectionErrorCode::DuplicateId,
            ),
            (
                "nested record list column",
                |policy| {
                    let nested = policy.facts[0].value_type.clone();
                    if let ValueType::RecordList { columns, .. } = &mut policy.facts[0].value_type {
                        columns.insert("nested".to_owned(), nested);
                    }
                },
                SelectionErrorCode::TypeMismatch,
            ),
        ];
        for (label, mutate, code) in cases {
            let mut policy = keyed_policy();
            mutate(&mut policy);
            let error = policy.validate().expect_err(label);
            assert_eq!(error.code(), code, "{label}: {error}");
        }
    }

    #[test]
    fn an_identity_domain_of_1024_members_is_declarable_and_1025_is_not() {
        let domain = |size: usize| ValueType::EnumString {
            allowed: (0..size)
                .map(|index| format!("member-{index:04}"))
                .collect(),
        };
        let mut policy = keyed_policy();
        policy.fields[0].value_type = domain(MAX_ENUM_VALUES);
        assert_eq!(MAX_ENUM_VALUES, 1_024);
        policy.validate().expect("a 1024-member domain validates");

        policy.fields[0].value_type = domain(MAX_ENUM_VALUES + 1);
        let error = policy.validate().expect_err("a 1025-member domain");
        assert_eq!(error.code(), SelectionErrorCode::LimitExceeded);
    }

    #[test]
    fn the_fixture_document_validates() {
        fixture().validate().expect("fixture policy validates");
    }

    #[test]
    fn a_foreign_schema_version_is_refused() {
        let mut policy = fixture();
        policy.schema_version = "actingcommand.selection-policy.v2".to_owned();
        let error = policy.validate().expect_err("foreign schema version");
        assert_eq!(error.code(), SelectionErrorCode::UnsupportedSchemaVersion);
    }

    #[test]
    fn an_undeclared_reference_is_a_dangling_reference() {
        let mut policy = fixture();
        policy.scoring[0].value = ValueRef::Field {
            field: "absent".to_owned(),
        };
        let error = policy.validate().expect_err("undeclared field");
        assert_eq!(error.code(), SelectionErrorCode::DanglingReference);

        let mut policy = fixture();
        policy.scoring[2].value = ValueRef::Fact {
            fact_key: "absent".to_owned(),
        };
        let error = policy.validate().expect_err("undeclared fact");
        assert_eq!(error.code(), SelectionErrorCode::DanglingReference);
    }

    #[test]
    fn a_non_integer_value_needs_a_lookup() {
        let mut policy = fixture();
        policy.scoring[1].transform = Transform::Identity;
        let error = policy.validate().expect_err("identity over a string");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    }

    #[test]
    fn a_lookup_key_outside_the_value_type_is_refused() {
        let mut policy = fixture();
        policy.scoring[1].transform = Transform::Lookup {
            entries: vec![LookupEntry {
                key: LookupKey::String("grade-absent".to_owned()),
                value_milli: 1,
            }],
            default_milli: None,
        };
        let error = policy.validate().expect_err("key outside the enum");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    }

    #[test]
    fn a_string_in_member_outside_the_value_type_is_refused() {
        let mut policy = fixture();
        let value = ValueRef::Field {
            field: "grade".to_owned(),
        };
        policy.gates[0].predicate = Predicate::StringIn {
            value: value.clone(),
            allowed: vec!["grade-hgih".to_owned()],
        };
        let error = policy.validate().expect_err("member outside the enum");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);

        policy.gates[0].predicate = Predicate::StringIn {
            value,
            allowed: vec!["grade-high".to_owned()],
        };
        policy.validate().expect("a declared member validates");
    }

    #[test]
    fn a_tie_break_key_over_an_enum_string_is_refused() {
        let mut policy = fixture();
        policy.tie_break[0] = TieBreakKey::Value {
            value: ValueRef::Field {
                field: "grade".to_owned(),
            },
            direction: SortDirection::HighestFirst,
        };
        let error = policy.validate().expect_err("enum_string tie-break");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);
    }

    #[test]
    fn repeated_identifiers_are_refused() {
        let mut policy = fixture();
        policy.scoring[1].term_id = policy.scoring[0].term_id.clone();
        let error = policy.validate().expect_err("repeated term_id");
        assert_eq!(error.code(), SelectionErrorCode::DuplicateId);

        let mut policy = fixture();
        policy.fields[1].name = policy.fields[0].name.clone();
        let error = policy.validate().expect_err("repeated field name");
        assert_eq!(error.code(), SelectionErrorCode::DuplicateId);
    }

    #[test]
    fn exactly_one_pins_its_required_count_and_top_k_needs_one() {
        let mut policy = fixture();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::ExactlyOne,
            required_count: 2,
        };
        let error = policy.validate().expect_err("exactly_one with two");
        assert_eq!(error.code(), SelectionErrorCode::TypeMismatch);

        let mut policy = fixture();
        policy.selection = SelectionRequirement {
            mode: SelectionMode::TopK,
            required_count: 0,
        };
        let error = policy.validate().expect_err("top_k with zero");
        assert_eq!(error.code(), SelectionErrorCode::MissingRequiredField);
    }

    #[test]
    fn declared_limits_are_enforced() {
        let mut policy = fixture();
        let term = policy.scoring[0].clone();
        policy.scoring = (0..=MAX_SCORING_TERMS)
            .map(|index| {
                let mut term = term.clone();
                term.term_id = format!("term-{index}");
                term
            })
            .collect();
        let error = policy.validate().expect_err("too many terms");
        assert_eq!(error.code(), SelectionErrorCode::LimitExceeded);

        let mut policy = fixture();
        let mut nested = policy.gates[0].predicate.clone();
        for _ in 0..MAX_PREDICATE_DEPTH {
            nested = Predicate::Not {
                of: Box::new(nested),
            };
        }
        policy.gates[0].predicate = nested;
        let error = policy.validate().expect_err("predicate too deep");
        assert_eq!(error.code(), SelectionErrorCode::LimitExceeded);
    }

    #[test]
    fn a_zero_freshness_bound_is_refused() {
        let mut policy = fixture();
        policy.facts[0].max_age_ms = 0;
        let error = policy.validate().expect_err("zero max_age_ms");
        assert_eq!(error.code(), SelectionErrorCode::MissingRequiredField);
    }

    #[test]
    fn an_unknown_document_field_is_refused_on_decode() {
        let source = include_str!("../tests/fixtures/policy.json").replacen(
            "\"policy_id\"",
            "\"unexpected\": 1,\n  \"policy_id\"",
            1,
        );
        serde_json::from_str::<SelectionPolicy>(&source).expect_err("unknown field");
    }
}
