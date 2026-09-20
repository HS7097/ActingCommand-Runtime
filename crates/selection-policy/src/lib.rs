// SPDX-License-Identifier: AGPL-3.0-only

//! Pure selection-policy contracts: one declared document, one deterministic evaluator.
//!
//! The crate answers exactly one question: given a bounded candidate set observed on one
//! screen, a declared selection-policy document, and an explicit fact snapshot, which
//! candidate identifiers are chosen and why. It owns the decision half only. It never
//! observes, never acts, never reads a clock, a ledger, a resource pack, or the filesystem;
//! every input is passed in and hashable, so the same input always yields the same output.
//!
//! Scores and lookup values are integers scaled by one thousand (milli). [`canonical_bytes`]
//! rejects floating point outright, so no document or input can smuggle one in.
//!
//! Absent, stale, low-confidence, and non-scalar facts stay [`UnknownReason`]-typed all the
//! way into the decision. Nothing here substitutes `false` or `0` for an unknown input; a
//! term or gate that meets one applies the handling its document declares, and the decision
//! records that it did.
//!
//! The `selection-eval` binary in `src/bin` is an offline debugging shell around
//! [`evaluate`]. It is the only place in this crate that reads a file, and it is not a
//! Runtime entry point.

#![forbid(unsafe_code)]

mod canonical;
mod evaluator;
mod facts;
mod schema;

pub use canonical::{
    CanonicalValue, MAX_DOCUMENT_BYTES, canonical_bytes, canonical_sha256, parse_canonical_json,
};
pub use evaluator::{
    CandidateStatus, CandidateVerdict, DecisionReason, GateOutcome, GateResult, SelectionDecision,
    SelectionOutcome, TermOutcome, TermResult, evaluate,
};
pub use facts::{Candidate, ScalarValue, SelectionFactEntry, SelectionFactSnapshot, UnknownReason};
pub use schema::{
    AppliesTo, FactDeclaration, FieldDeclaration, GateUnknownHandling, HardGate, LookupEntry,
    LookupKey, MAX_CANDIDATES, MAX_ENUM_VALUES, MAX_FACTS, MAX_FIELDS, MAX_GATES, MAX_ID_BYTES,
    MAX_LOOKUP_ENTRIES, MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES, MAX_SCORING_TERMS,
    MAX_TIE_BREAK_KEYS, OutcomeKeys, Predicate, SELECTION_POLICY_SCHEMA_VERSION, ScoringTerm,
    SelectionError, SelectionErrorCode, SelectionMode, SelectionPolicy, SelectionRequirement,
    SortDirection, TermUnknownHandling, TieBreakKey, Transform, ValueRef, ValueType,
};
