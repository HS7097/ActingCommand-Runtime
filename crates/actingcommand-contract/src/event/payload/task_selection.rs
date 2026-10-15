// SPDX-License-Identifier: AGPL-3.0-only

// The record of one in-task selection (Workflow #308, `task.selection_evaluated`).
//
// A select step records exactly one of these per attempt: the candidate projection it
// evaluated, the identities of the policy and of the inputs, the full per-candidate breakdown,
// the choice, and how the confirmation frame compared. A single select records it before any
// input and before any failure return; a multi select (M-357b §5.3) records it once its tap
// sequence ends, fails or stops, before the step's `task.effect_completed`. The breakdown
// types mirror the selection-policy evaluator's decision shapes field for field
// (`contracts/selection-graph.md`, section Records); this crate does not depend on the
// evaluator, and the kernel converts one into the other.
//
// A scan select (frozen model S0 v2.1 §1.9, slice SEL-L4a) records the scan-level projection v2,
// its own confirmation variants and `unknown_check`, and the record is checked against the
// path table of S0 §3.4 through the seven path rules of §1.9.
//
// A multi select (Workflow #357, frozen model M-357b v1.0 §5, slice DB-3s) chooses up to N
// candidates in one attempt and records them in `multi_select`, with one confirmation and one
// tap check per pick; the record is checked by rules M1–M8 of M-357b §5.2 on top of the path
// rules. DB-3s writes the shape, its readers and its rules, never a writer.

use super::{
    PolicyReasonRecord, validate_policy_digest, validate_policy_token, validate_task_semantic_label,
};
use crate::candidate_projection::is_business_identity;
use crate::outcome::Outcome;
use crate::{
    CANDIDATE_PROJECTION_SCHEMA_VERSION, CandidateProjection, CandidateProjectionError,
    CandidateUnknownReason, SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION, SanitizationError,
    ScanCandidateProjection, ScanCoverageState, parse_candidate_id,
    validate_candidate_feature_name,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::collections::BTreeSet;

/// Candidates one record may break down; the projection's own candidate budget.
pub const TASK_SELECTION_MAX_CANDIDATES: usize = crate::CANDIDATE_PROJECTION_MAX_CANDIDATES;
/// Compact JSON bytes of one record.
pub const TASK_SELECTION_RECORD_MAX_BYTES: usize = 64 * 1024;
/// Entries of the decision-level reason chain, as for a policy dispatch.
pub const TASK_SELECTION_MAX_REASONS: usize = 128;
/// Picks one multi-select record may hold (M-357b §5.1).
pub const TASK_SELECTION_MAX_PICKS: usize = 16;
/// The least count a multi-select policy declares (M-357b §1.2).
pub const TASK_SELECTION_MIN_DECLARED_COUNT: u32 = 2;
/// The largest count a multi-select policy declares (M-357b §1.2).
pub const TASK_SELECTION_MAX_DECLARED_COUNT: u32 = 16;
/// The largest free-slot reading a multi-select record carries (M-357b §3.6).
pub const TASK_SELECTION_MAX_FREE_SLOTS: u32 = 65_535;

/// M-357b §3.4 and rule M8: the decision-level reason a combination quota left unfilled.
/// DB-3a2 registers the code in its owner's registry, which this crate does not link, so the
/// reader names it by its spelling.
const SELECTION_QUOTA_UNFILLED: &str = "selection_quota_unfilled";
/// The record's compact JSON exceeds [`TASK_SELECTION_RECORD_MAX_BYTES`].
pub const SELECTION_RECORD_TOO_LARGE: crate::outcome::Code =
    crate::codes::SanitizationCode::SelectionRecordTooLarge.code();

const INVALID: crate::outcome::Code =
    crate::codes::SanitizationCode::InvalidTaskSelectionRecord.code();

/// The selection policy the step evaluated. Workflow #336 PKG-S2 (option 丙): no pin the step
/// may declare for the document is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionPolicy {
    /// The package-relative document path the step declares.
    pub path: String,
    /// The evaluator's canonical identity of the parsed document, `sha256:<hex>`.
    pub policy_sha256: String,
    pub policy_id: String,
}

/// Mirrors the evaluator's `UnknownReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionUnknownReason {
    FactMissing,
    FactExpired,
    FactStale,
    FactLowConfidence,
    FactNotScalar,
    FieldMissing,
    TypeMismatch,
    LookupMiss,
}

/// Mirrors the evaluator's `SelectionOutcome`; an unknown outcome keeps its reason, and the
/// evaluator's text goes only to the developer log (ledger revision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionOutcome {
    Selected { count: u32 },
    Empty,
    Insufficient { surviving: u32, required: u32 },
    Ambiguous { candidate_ids: Vec<String> },
    Unknown { reason: TaskSelectionUnknownReason },
}

/// Mirrors the evaluator's `GateOutcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionGateOutcome {
    Passed,
    Failed,
    UnknownSubstituted {
        reason: TaskSelectionUnknownReason,
        passes: bool,
    },
    UnknownDropped {
        reason: TaskSelectionUnknownReason,
    },
}

/// Mirrors the evaluator's `GateResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionGateResult {
    pub gate_id: String,
    pub selection_result: TaskSelectionGateOutcome,
}

/// Mirrors the evaluator's `TermOutcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionTermOutcome {
    Scored {
        transformed_milli: i64,
    },
    UnknownSubstituted {
        reason: TaskSelectionUnknownReason,
        transformed_milli: i64,
    },
    UnknownDropped {
        reason: TaskSelectionUnknownReason,
    },
}

/// Mirrors the evaluator's `TermResult`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionTermResult {
    pub term_id: String,
    pub selection_result: TaskSelectionTermOutcome,
    pub weight_milli: i64,
    pub contribution_milli: i64,
}

/// Mirrors the evaluator's `CandidateStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionCandidateStatus {
    Ranked,
    GateRejected,
    UnknownDropped,
}

/// Mirrors the evaluator's `CandidateVerdict`; its reasons use the policy reason record, each
/// the outcome of an evaluator `DecisionReason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionVerdict {
    pub candidate_id: String,
    pub status: TaskSelectionCandidateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_milli: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    pub gates: Vec<TaskSelectionGateResult>,
    pub terms: Vec<TaskSelectionTermResult>,
    pub reasons: Vec<PolicyReasonRecord>,
}

/// What a scan select's confirmation frame F2 held at the chosen position when its identity was
/// not confirmed: no instance, only Unknown readings until the settle timeout, or another
/// business ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionIdentityState {
    Absent,
    Unknown,
    Different,
}

/// Where a scan select's repositioning failed: returning to the target, aligning F2 against
/// the content map, or aligning a multi select's frame after a tap (M-357b §4.3; only a v2
/// pick's tap check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionRepositionStage {
    Return,
    Confirm,
    TapCheck,
}

/// The order a multi select taps its picks in: by rank, or by candidate index (M-357b §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionPickOrder {
    Rank,
    Content,
}

/// What a multi select's mark feature read on a frame (M-357b §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionMarkState {
    Marked,
    Unmarked,
    Unknown,
}

/// What a multi select's mark feature had to read on a frame (M-357b §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionMarkExpectation {
    Marked,
    Unmarked,
}

impl TaskSelectionMarkExpectation {
    /// Whether `state` is the reading this expectation asks for.
    fn met_by(self, state: TaskSelectionMarkState) -> bool {
        matches!(
            (self, state),
            (Self::Marked, TaskSelectionMarkState::Marked)
                | (Self::Unmarked, TaskSelectionMarkState::Unmarked)
        )
    }
}

/// How the confirmation frame compared with the evaluated projection.
///
/// A confirmation is attempted exactly when the outcome is `selected` and nothing blocks it
/// (path rule 7). `matched` and `mismatched` carry the confirmation frame's candidate-set hash
/// and belong to a single-frame select (v1); `identity_matched`, `identity_unconfirmed` and
/// `reposition_failed` belong to a scan select (v2).
///
/// A multi select (M-357b §5.1) adds six variants. `pick_matched`, `mark_unconfirmed` and
/// `tap_marked` appear only inside `multi_select.picks`, `pick_moved` only as a v1 pick's
/// confirmation or tap check, and `picks_confirmed` and `picks_stopped` only as a multi-select
/// record's top-level confirmation. A record without `multi_select` carries none of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionConfirmation {
    NotAttempted,
    Matched {
        candidate_set_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<Box<CandidateProjection>>,
    },
    Mismatched {
        candidate_set_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<Box<CandidateProjection>>,
    },
    PageChanged,
    /// The guard's failure (ledger revision 3; was `code`).
    GuardFailed {
        outcome: Outcome,
    },
    /// The confirmation capture's failure (ledger revision 3; was `code`). A scan select's F2
    /// capture and settle failures are recorded here too.
    CaptureFailed {
        outcome: Outcome,
    },
    /// Scan select: F2's instance at the chosen candidate's content position carried the chosen
    /// business ID. `frame_id` is F2 and `content_offset_px` its aligned offset; F2's projection
    /// is not stored.
    IdentityMatched {
        candidate_id: String,
        frame_id: crate::FrameId,
        content_offset_px: i32,
    },
    /// Scan select: F2's identity at the chosen position was not the chosen business ID.
    /// `observed_identity` is present exactly when the state is `different`, and
    /// `identity_unknown` may say why an `unknown` reading was Unknown.
    IdentityUnconfirmed {
        identity_state: TaskSelectionIdentityState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_identity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_unknown: Option<CandidateUnknownReason>,
    },
    /// Scan select: the return to the target or F2's alignment against the content map failed;
    /// the failure travels as `outcome`, as for `guard_failed` (ledger revision 3). A multi
    /// select's tap check records stage `tap_check` when its frame does not align.
    RepositionFailed {
        outcome: Outcome,
        stage: TaskSelectionRepositionStage,
    },
    /// Single-frame multi select: the frame `frame_id` confirmed a pick before its tap
    /// (M-357b §4.2).
    PickMatched {
        frame_id: crate::FrameId,
    },
    /// Single-frame multi select: the unmarked hash of `frame_id`, `pick_set_sha256`, differs
    /// from the decision frame's, so the list moved; before a tap or in a tap check.
    PickMoved {
        frame_id: crate::FrameId,
        pick_set_sha256: String,
    },
    /// Multi select: on `frame_id`, the mark of `candidate_id` read `mark_state` where
    /// `mark_expectation` was required; before a tap or in a tap check.
    MarkUnconfirmed {
        candidate_id: String,
        mark_expectation: TaskSelectionMarkExpectation,
        mark_state: TaskSelectionMarkState,
        frame_id: crate::FrameId,
    },
    /// Multi select: the frame `frame_id` after a tap read the tapped pick as marked.
    TapMarked {
        frame_id: crate::FrameId,
    },
    /// Multi select, top level only: every pick was confirmed, tapped and checked.
    PicksConfirmed {
        count: u32,
    },
    /// Multi select, top level only: the sequence stopped outside the record (a deadline, the
    /// task timeout, an input failure or the run's end) after `count` picks were tapped and
    /// checked (M-357b §4.5).
    PicksStopped {
        count: u32,
    },
}

/// The projection a select step evaluated, tagged by its `schema_version`: v1, one frame, for a
/// single-frame select; v2, the scan, for a scan select. It serialises as the projection itself,
/// so a v1 record keeps its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TaskSelectionProjection {
    V1(CandidateProjection),
    V2(Box<ScanCandidateProjection>),
}

impl TaskSelectionProjection {
    pub fn schema_version(&self) -> &str {
        match self {
            Self::V1(projection) => projection.schema_version(),
            Self::V2(projection) => projection.schema_version(),
        }
    }

    pub fn page_id(&self) -> &str {
        match self {
            Self::V1(projection) => projection.page_id(),
            Self::V2(projection) => projection.page_id(),
        }
    }

    pub fn layout_id(&self) -> &str {
        match self {
            Self::V1(projection) => projection.layout_id(),
            Self::V2(projection) => projection.layout_id(),
        }
    }

    pub fn candidate_set_sha256(&self) -> &str {
        match self {
            Self::V1(projection) => projection.candidate_set_sha256(),
            Self::V2(projection) => projection.candidate_set_sha256(),
        }
    }

    /// Whether the evaluator may hold a verdict for this candidate: an actionable v1 candidate,
    /// or a v2 member, every one of which is actionable.
    fn evaluates(&self, candidate_id: &str) -> bool {
        match self {
            Self::V1(projection) => projection
                .candidate(candidate_id)
                .is_some_and(|candidate| candidate.actionable),
            Self::V2(projection) => projection.candidate(candidate_id).is_some(),
        }
    }

    fn validate_decoded(&self) -> Result<(), CandidateProjectionError> {
        match self {
            Self::V1(projection) => projection.validate_decoded(),
            Self::V2(projection) => projection.validate_decoded(),
        }
    }

    fn validate_encoded_size(&self) -> Result<(), CandidateProjectionError> {
        match self {
            Self::V1(projection) => projection.validate_encoded_size(),
            Self::V2(projection) => projection.validate_encoded_size(),
        }
    }
}

impl From<CandidateProjection> for TaskSelectionProjection {
    fn from(projection: CandidateProjection) -> Self {
        Self::V1(projection)
    }
}

impl From<ScanCandidateProjection> for TaskSelectionProjection {
    fn from(projection: ScanCandidateProjection) -> Self {
        Self::V2(Box::new(projection))
    }
}

impl<'de> Deserialize<'de> for TaskSelectionProjection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
        {
            Some(CANDIDATE_PROJECTION_SCHEMA_VERSION) => serde_json::from_value(value)
                .map(Self::V1)
                .map_err(de::Error::custom),
            Some(SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION) => serde_json::from_value(value)
                .map(|projection| Self::V2(Box::new(projection)))
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom(
                "a selection projection's schema_version names candidate projection v1 or v2",
            )),
        }
    }
}

/// Why a decision that unread positions could change did not stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionBlocking {
    None,
    /// `empty` or `insufficient` with at least one unknown instance (rule 2 of S0 §3.2).
    UnknownInstances,
    /// `selected`, and an unknown instance might outrank the choice (rule 3 of S0 §3.2).
    OutrankedPossible,
}

/// The largest score any unknown instance could reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionUpperBound {
    /// There is no unknown instance.
    NoneUnknown,
    /// No unknown instance could pass the gates.
    NoneEligible,
    Max {
        milli: i64,
    },
    Unbounded,
}

/// Why an `empty`, `insufficient` or `selected` decision did or did not stand. Present exactly
/// when the step declares `scan` or `on_empty`; the evaluator's decision stays verbatim in
/// `selection_result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionUnknownCheck {
    pub unknown_instances: u32,
    pub blocking: TaskSelectionBlocking,
    pub upper_bound: TaskSelectionUpperBound,
}

/// One pick of a multi select, in tap order: the confirmation before its tap, and the check
/// after it, present only when that check reached a result (M-357b §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSelectionPick {
    pub candidate_id: String,
    pub confirmation: TaskSelectionConfirmation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap_check: Option<TaskSelectionConfirmation>,
}

/// A multi select's counts, its step's declarations and its picks (M-357b §5.1). Present
/// exactly when the step's policy declares more than one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionMultiSelect {
    /// The policy's `selection.required_count`, 2..=16.
    pub declared_count: u32,
    /// The resolved count N: min(`declared_count`, `free_slots`), else `declared_count`.
    pub required_count: u32,
    /// The free-slot fact's reading, 1..=65535; absent when the policy declares none or the
    /// reading was unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_slots: Option<u32>,
    pub order: TaskSelectionPickOrder,
    /// The step's mark feature: a feature name of the layout.
    pub mark_feature: String,
    /// The decision frame's unmarked hash, 64 lowercase hex digits; present exactly with a v1
    /// projection. Its definition is the writer's (M-357b §4.2); a reader checks its grammar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_set_sha256: Option<String>,
    pub picks: Vec<TaskSelectionPick>,
}

/// One select step's decision record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionRecord {
    pub layout_id: String,
    pub page_id: String,
    /// The core projection the step evaluated, whole: of its frame (v1), or of its scan (v2).
    pub projection: TaskSelectionProjection,
    pub policy: TaskSelectionPolicy,
    pub fact_snapshot_id: String,
    /// Absent (read as 0) when the record carries `input_fact_revisions` instead (Workflow #383,
    /// register L9); a present position is never 0.
    #[serde(
        default,
        skip_serializing_if = "super::observer::input_ledger_position_absent",
        deserialize_with = "super::observer::deserialize_input_ledger_position"
    )]
    pub input_ledger_position: u64,
    /// Workflow #383 (register L9): both stores' (lineage, revision) the selection read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_fact_revisions: Option<super::InputFactRevisions>,
    pub now_unix_ms: u64,
    /// The evaluator's canonical identity of candidates, facts and instant, `sha256:<hex>`.
    pub input_sha256: String,
    pub selection_result: TaskSelectionOutcome,
    pub outcome_key: String,
    pub selected: Vec<String>,
    pub verdicts: Vec<TaskSelectionVerdict>,
    pub reasons: Vec<PolicyReasonRecord>,
    pub confirmation: TaskSelectionConfirmation,
    /// Present exactly when the step declares `scan` or `on_empty`; every v2 record has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_check: Option<TaskSelectionUnknownCheck>,
    /// Present exactly when the step's policy declares more than one candidate (M-357b §5.1);
    /// a single select's record never carries it and keeps its bytes.
    #[serde(default)]
    pub multi_select: Option<TaskSelectionMultiSelect>,
}

impl TaskSelectionRecord {
    /// Structural checks, run on write and on read. Nothing is re-measured here.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.projection
            .validate_decoded()
            .map_err(projection_error)?;
        if self.layout_id != self.projection.layout_id() {
            return Err(invalid("layout_id"));
        }
        if self.page_id != self.projection.page_id() {
            return Err(invalid("page_id"));
        }
        self.policy.validate()?;
        validate_policy_token(&self.fact_snapshot_id, "fact_snapshot_id")?;
        if self.input_ledger_position == 0 && self.input_fact_revisions.is_none() {
            return Err(invalid("input_ledger_position"));
        }
        if self.now_unix_ms == 0 {
            return Err(invalid("now_unix_ms"));
        }
        validate_policy_digest(&self.input_sha256, "input_sha256")?;
        self.selection_result.validate()?;
        validate_task_semantic_label(&self.outcome_key, "outcome_key")?;

        if self.verdicts.len() > TASK_SELECTION_MAX_CANDIDATES {
            return Err(invalid("verdicts"));
        }
        let mut evaluated = BTreeSet::new();
        for verdict in &self.verdicts {
            verdict.validate()?;
            if !self.projection.evaluates(&verdict.candidate_id)
                || !evaluated.insert(verdict.candidate_id.as_str())
            {
                return Err(invalid("verdicts"));
            }
        }

        let expected = match self.selection_result {
            TaskSelectionOutcome::Selected { count } => count as usize,
            _ => 0,
        };
        if self.selected.len() != expected {
            return Err(invalid("selected"));
        }
        let mut chosen = BTreeSet::new();
        for candidate_id in &self.selected {
            let ranked = self.verdicts.iter().any(|verdict| {
                verdict.candidate_id == *candidate_id
                    && verdict.status == TaskSelectionCandidateStatus::Ranked
            });
            if !ranked || !chosen.insert(candidate_id.as_str()) {
                return Err(invalid("selected"));
            }
        }

        if self.reasons.is_empty() || self.reasons.len() > TASK_SELECTION_MAX_REASONS {
            return Err(invalid("reasons"));
        }
        validate_reasons(&self.reasons)?;
        if let Some(multi) = &self.multi_select {
            self.validate_multi_select(multi)?;
        }
        self.validate_paths()
    }

    /// [`Self::validate`] plus the byte budgets, measured on the current encoding: the
    /// projection within its own budget and the whole record within
    /// [`TASK_SELECTION_RECORD_MAX_BYTES`] (`selection_record_too_large`). Write side only; a
    /// single select's producer runs it before any input, and a multi select's runs it once
    /// on placeholder picks before its first tap (M-357b §5.3).
    pub fn validate_for_append(&self) -> Result<(), SanitizationError> {
        self.validate()?;
        self.projection
            .validate_encoded_size()
            .map_err(projection_error)?;
        if let TaskSelectionConfirmation::Matched {
            projection: Some(projection),
            ..
        }
        | TaskSelectionConfirmation::Mismatched {
            projection: Some(projection),
            ..
        } = &self.confirmation
        {
            projection
                .validate_encoded_size()
                .map_err(projection_error)?;
        }
        let bytes = serde_json::to_vec(self).map_err(|_| invalid("selection"))?;
        if bytes.len() > TASK_SELECTION_RECORD_MAX_BYTES {
            return Err(SanitizationError::new(
                SELECTION_RECORD_TOO_LARGE,
                "selection",
            ));
        }
        Ok(())
    }

    /// The path rules of S0 §1.9, which tie `selection_result` (R), the v2 coverage state (C),
    /// `unknown_check.blocking` (B) and the confirmation to the §3.4 path table. A v2 record needs
    /// `unknown_check`. A v1 record with `unknown_check` (a single-frame select that declares
    /// `on_empty`) keeps rules 3–7 without their `unknown[]` and `verdicts` clauses; a v1 record
    /// without one keeps rule 7 alone, with nothing blocking. Rule 2 is the v2 projection's own
    /// (`ScanCandidateProjection`, refused as `viewports`); a refusal of rule 7 names
    /// `confirmation`, every other rule names itself. A multi-select record reads rules 1, 4
    /// and 5 as M-357b's M8, M6 and M7, with N its `required_count`.
    fn validate_paths(&self) -> Result<(), SanitizationError> {
        let scan = match &self.projection {
            TaskSelectionProjection::V1(_) => None,
            TaskSelectionProjection::V2(projection) => Some(&**projection),
        };
        let Some(check) = self.unknown_check else {
            if scan.is_some() {
                return Err(invalid("unknown_check"));
            }
            return self.validate_confirmation(true, false);
        };
        let result = &self.selection_result;
        let selected = matches!(result, TaskSelectionOutcome::Selected { .. });
        let unknown = matches!(result, TaskSelectionOutcome::Unknown { .. });
        let blocking_none = check.blocking == TaskSelectionBlocking::None;
        let multi = self.multi_select.as_ref();
        // A single select chooses exactly one candidate, whose verdict carries the chosen score;
        // a multi select chooses up to N (M1).
        if multi.is_none() && selected && self.selected.len() != 1 {
            return Err(invalid("selected"));
        }
        // A multi select's `selected` holds all N candidates, or fewer (a partial fill).
        let full = multi.is_none_or(|multi| self.selected.len() == multi.required_count as usize);

        // Rule 1: R unknown ⇔ C not_started or incomplete; R empty, insufficient or ambiguous ⇒
        // C complete; R selected ⇒ C complete or early_stopped; C early_stopped ⇒ R selected and
        // B none. M8: a multi select stops early only full and with every quota filled.
        if let Some(scan) = scan {
            let state = scan.coverage().state;
            let unfinished = matches!(
                state,
                ScanCoverageState::NotStarted | ScanCoverageState::Incomplete
            );
            let not_selected = matches!(
                result,
                TaskSelectionOutcome::Empty
                    | TaskSelectionOutcome::Insufficient { .. }
                    | TaskSelectionOutcome::Ambiguous { .. }
            );
            let holds = unknown == unfinished
                && (!not_selected || state == ScanCoverageState::Complete)
                && (!selected
                    || matches!(
                        state,
                        ScanCoverageState::Complete | ScanCoverageState::EarlyStopped
                    ))
                && (state != ScanCoverageState::EarlyStopped || (selected && blocking_none));
            let early_stop_holds = multi.is_none()
                || state != ScanCoverageState::EarlyStopped
                || (full
                    || !self
                        .reasons
                        .iter()
                        .any(|reason| reason.0.code().as_str() == SELECTION_QUOTA_UNFILLED));
            if !holds || !early_stop_holds {
                return Err(invalid("path_rule_1"));
            }
        }

        // Rule 3: `unknown_instances` = len(unknown[]) (v2); the bound is `none_unknown` ⇔ there
        // is no unknown instance.
        let unknown_instances = check.unknown_instances;
        if scan.is_some_and(|scan| scan.unknown().len() != unknown_instances as usize)
            || (check.upper_bound == TaskSelectionUpperBound::NoneUnknown)
                != (unknown_instances == 0)
        {
            return Err(invalid("path_rule_3"));
        }

        // Rule 4: B unknown_instances ⇔ R empty or insufficient with an unknown instance. M6: or
        // a multi select's partial `selected` with an unknown instance.
        let unread_blocks = (matches!(
            result,
            TaskSelectionOutcome::Empty | TaskSelectionOutcome::Insufficient { .. }
        ) || (selected && !full && false))
            && unknown_instances >= 1;
        if (check.blocking == TaskSelectionBlocking::UnknownInstances) != unread_blocks {
            return Err(invalid("path_rule_4"));
        }

        // Rule 5: B outranked_possible ⇔ R selected and the bound is unbounded, or a maximum
        // above the chosen score. M7: a multi select's `selected` must be full, and the chosen
        // score is the lowest selected score; a partial fill is never outranked_possible.
        let outranked = selected
            && match check.upper_bound {
                TaskSelectionUpperBound::Unbounded => true,
                TaskSelectionUpperBound::Max { milli } => {
                    milli > self.chosen_score().ok_or_else(|| invalid("path_rule_5"))?
                }
                TaskSelectionUpperBound::NoneUnknown | TaskSelectionUpperBound::NoneEligible => {
                    false
                }
            };
        if (check.blocking == TaskSelectionBlocking::OutrankedPossible) != outranked {
            return Err(invalid("path_rule_5"));
        }

        // Rule 6: R unknown ⇒ B none and, in v2, no verdict.
        if unknown && (!blocking_none || (scan.is_some() && !self.verdicts.is_empty())) {
            return Err(invalid("path_rule_6"));
        }

        // Rule 7.
        self.validate_confirmation(blocking_none, scan.is_some())
    }

    /// Path rule 7: a confirmation is attempted ⇔ R selected and B none. A v2 record never
    /// records `matched` or `mismatched`, a v1 record never one of the three scan variants, and
    /// `identity_matched` names the selected candidate. A single select never records a
    /// multi-select variant or a tap check's reposition stage; a multi select's confirmation
    /// follows its picks (M-357b M2–M5).
    fn validate_confirmation(
        &self,
        blocking_none: bool,
        scan: bool,
    ) -> Result<(), SanitizationError> {
        let selected = matches!(self.selection_result, TaskSelectionOutcome::Selected { .. });
        let attempted = !matches!(self.confirmation, TaskSelectionConfirmation::NotAttempted);
        if attempted != (selected && blocking_none) {
            return Err(invalid("confirmation"));
        }
        if let Some(multi) = &self.multi_select {
            return self.validate_picks(multi, scan);
        }
        match &self.confirmation {
            TaskSelectionConfirmation::NotAttempted
            | TaskSelectionConfirmation::PageChanged
            | TaskSelectionConfirmation::GuardFailed { .. }
            | TaskSelectionConfirmation::CaptureFailed { .. } => Ok(()),
            TaskSelectionConfirmation::Matched { .. }
            | TaskSelectionConfirmation::Mismatched { .. } => self.validate_frame_comparison(),
            TaskSelectionConfirmation::IdentityMatched { candidate_id, .. } => {
                if !scan || self.selected.first() != Some(candidate_id) {
                    return Err(invalid("confirmation"));
                }
                Ok(())
            }
            TaskSelectionConfirmation::IdentityUnconfirmed {
                identity_state,
                observed_identity,
                identity_unknown,
            } => {
                let consistent = identity_unconfirmed_consistent(
                    *identity_state,
                    observed_identity.as_deref(),
                    identity_unknown.is_some(),
                );
                if !scan || !consistent {
                    return Err(invalid("confirmation"));
                }
                Ok(())
            }
            TaskSelectionConfirmation::RepositionFailed { stage, .. } => {
                if !scan || *stage == TaskSelectionRepositionStage::TapCheck {
                    return Err(invalid("confirmation"));
                }
                Ok(())
            }
            // M-357b §5.2: the multi-select variants belong to a record with `multi_select`.
            TaskSelectionConfirmation::PickMatched { .. }
            | TaskSelectionConfirmation::PickMoved { .. }
            | TaskSelectionConfirmation::MarkUnconfirmed { .. }
            | TaskSelectionConfirmation::TapMarked { .. }
            | TaskSelectionConfirmation::PicksConfirmed { .. }
            | TaskSelectionConfirmation::PicksStopped { .. } => Ok(()),
        }
    }

    /// M-357b rule M1 (field `multi_select`): the counts, the resolved count against the free
    /// slots, the decision against N, the mark feature's name and the unmarked hash's presence
    /// and grammar. A reader never recomputes the hash (D29).
    fn validate_multi_select(
        &self,
        multi: &TaskSelectionMultiSelect,
    ) -> Result<(), SanitizationError> {
        let declared = multi.declared_count;
        let required = multi.required_count;
        let resolved = match multi.free_slots {
            Some(free_slots) => (1..=TASK_SELECTION_MAX_FREE_SLOTS)
                .contains(&free_slots)
                .then_some(declared.min(free_slots)),
            None => Some(declared),
        };
        let counts = (TASK_SELECTION_MIN_DECLARED_COUNT..=TASK_SELECTION_MAX_DECLARED_COUNT)
            .contains(&declared)
            && (1..=declared).contains(&required)
            && resolved.is_some();
        let decision = match self.selection_result {
            TaskSelectionOutcome::Ambiguous { .. } => false,
            TaskSelectionOutcome::Insufficient {
                required: wanted, ..
            } => wanted <= required,
            TaskSelectionOutcome::Selected { .. }
            | TaskSelectionOutcome::Empty
            | TaskSelectionOutcome::Unknown { .. } => true,
        } && self.selected.len() <= required as usize;
        let hash = match (&self.projection, &multi.pick_set_sha256) {
            (TaskSelectionProjection::V1(_), Some(hash)) => is_candidate_set_sha256(hash),
            (TaskSelectionProjection::V2(_), None) => true,
            (TaskSelectionProjection::V1(_), None) | (TaskSelectionProjection::V2(_), Some(_)) => {
                false
            }
        };
        if !counts
            || !decision
            || !hash
            || validate_candidate_feature_name(&multi.mark_feature).is_err()
        {
            return Err(invalid("multi_select"));
        }
        Ok(())
    }

    /// M-357b rules M2–M5. A record whose top-level confirmation was not attempted holds no
    /// pick (M5). Otherwise its picks are exactly `selected`, in rank order or in candidate
    /// index order (M2); each pick's confirmation and tap check is one the projection allows
    /// (M3); the picks succeed up to a first pick j that failed or stopped, and every later
    /// pick was not attempted (M4); and the top-level confirmation is `picks_confirmed` when
    /// none failed or stopped, j's failure verbatim when it failed, or `picks_stopped` with j
    /// when it stopped (M5). Refusals of M2–M4 name `picks`, of M5 `confirmation`.
    fn validate_picks(
        &self,
        multi: &TaskSelectionMultiSelect,
        scan: bool,
    ) -> Result<(), SanitizationError> {
        let picks = &multi.picks;
        if matches!(self.confirmation, TaskSelectionConfirmation::NotAttempted) {
            return if picks.is_empty() {
                Ok(())
            } else {
                Err(invalid("confirmation"))
            };
        }

        // M2.
        if picks.len() > TASK_SELECTION_MAX_PICKS || picks.len() != self.selected.len() {
            return Err(invalid("picks"));
        }
        let mut seen = BTreeSet::new();
        for pick in picks {
            if !self.selected.contains(&pick.candidate_id)
                || !seen.insert(pick.candidate_id.as_str())
            {
                return Err(invalid("picks"));
            }
        }
        let ordered = match multi.order {
            TaskSelectionPickOrder::Rank => picks
                .iter()
                .zip(&self.selected)
                .all(|(pick, chosen)| pick.candidate_id == *chosen),
            TaskSelectionPickOrder::Content => {
                let mut indices = Vec::with_capacity(picks.len());
                for pick in picks {
                    let (_, index) =
                        parse_candidate_id(&pick.candidate_id).map_err(|_| invalid("picks"))?;
                    indices.push(index);
                }
                indices.windows(2).all(|pair| pair[0] != pair[1])
            }
        };
        if !ordered {
            return Err(invalid("picks"));
        }

        // M3 and M4: the first pick that did not succeed ends the sequence.
        let mut end: Option<PickEnd<'_>> = None;
        let mut previous: Option<&TaskSelectionPick> = None;
        for (index, pick) in picks.iter().enumerate() {
            if !self.pick_confirmation_allowed(pick, previous, scan)
                || pick
                    .tap_check
                    .as_ref()
                    .is_some_and(|tap_check| !self.tap_check_allowed(tap_check, scan))
            {
                return Err(invalid("picks"));
            }
            previous = Some(pick);
            let not_attempted =
                matches!(pick.confirmation, TaskSelectionConfirmation::NotAttempted);
            if end.is_some() {
                continue;
            }
            if end.is_some() {
                if !not_attempted || pick.tap_check.is_some() {
                    return Err(invalid("picks"));
                }
                continue;
            }
            let confirmed = matches!(
                pick.confirmation,
                TaskSelectionConfirmation::PickMatched { .. }
                    | TaskSelectionConfirmation::IdentityMatched { .. }
            );
            end = match (&pick.tap_check, confirmed) {
                (Some(TaskSelectionConfirmation::TapMarked { .. }), true) => None,
                // (b) the tap check failed.
                (Some(tap_check), true) => Some(PickEnd::Failed(tap_check)),
                // (c) tapped or about to be, then stopped before the tap check's result.
                (None, true) => Some(PickEnd::Stopped(index)),
                // (d) stopped before its confirmation.
                (None, false) if not_attempted => Some(PickEnd::Stopped(index)),
                // (a) the confirmation failed.
                (None, false) => Some(PickEnd::Failed(&pick.confirmation)),
                // A failed or skipped confirmation has no tap check.
                (Some(_), false) => Some(PickEnd::Failed(&pick.confirmation)),
            };
        }

        // M5.
        let expected = match end {
            None => TaskSelectionConfirmation::PicksConfirmed {
                count: picks.len() as u32,
            },
            Some(PickEnd::Failed(failure)) => failure.clone(),
            Some(PickEnd::Stopped(index)) => TaskSelectionConfirmation::PicksStopped {
                count: index as u32,
            },
        };
        if std::mem::discriminant(&self.confirmation) != std::mem::discriminant(&expected) {
            return Err(invalid("confirmation"));
        }
        Ok(())
    }

    /// M3, a pick's confirmation before its tap. A v1 pick confirms on a frame (the first on
    /// F2, every later one on its predecessor's tap-check frame, so only the first can fail
    /// before a tap); a v2 pick returns to its position and confirms the identity there.
    fn pick_confirmation_allowed(
        &self,
        pick: &TaskSelectionPick,
        previous: Option<&TaskSelectionPick>,
        scan: bool,
    ) -> bool {
        let confirmation = &pick.confirmation;
        match confirmation {
            TaskSelectionConfirmation::NotAttempted => true,
            TaskSelectionConfirmation::PickMatched { frame_id } => {
                !scan
                    && previous.is_none_or(|previous| {
                        matches!(
                            &previous.tap_check,
                            Some(TaskSelectionConfirmation::TapMarked { frame_id: tapped })
                                if tapped == frame_id
                        )
                    })
            }
            TaskSelectionConfirmation::PickMoved {
                pick_set_sha256, ..
            } => !scan && is_candidate_set_sha256(pick_set_sha256),
            TaskSelectionConfirmation::PageChanged
            | TaskSelectionConfirmation::GuardFailed { .. }
            | TaskSelectionConfirmation::CaptureFailed { .. } => scan || previous.is_none(),
            TaskSelectionConfirmation::MarkUnconfirmed { .. } => {
                (scan || previous.is_none()) && self.mark_unconfirmed_allowed(confirmation, scan)
            }
            TaskSelectionConfirmation::IdentityMatched { candidate_id, .. } => {
                scan && *candidate_id == pick.candidate_id
            }
            TaskSelectionConfirmation::IdentityUnconfirmed {
                identity_state,
                observed_identity,
                identity_unknown,
            } => {
                scan && identity_unconfirmed_consistent(
                    *identity_state,
                    observed_identity.as_deref(),
                    identity_unknown.is_some(),
                )
            }
            TaskSelectionConfirmation::RepositionFailed { stage, .. } => {
                scan && *stage != TaskSelectionRepositionStage::TapCheck
            }
            TaskSelectionConfirmation::Matched { .. }
            | TaskSelectionConfirmation::Mismatched { .. }
            | TaskSelectionConfirmation::TapMarked { .. }
            | TaskSelectionConfirmation::PicksConfirmed { .. }
            | TaskSelectionConfirmation::PicksStopped { .. } => false,
        }
    }

    /// M3, a pick's check after its tap: the tapped pick marked, or the failure that frame
    /// showed; a v1 frame may also show the list moved, a v2 frame may fail to align.
    fn tap_check_allowed(&self, tap_check: &TaskSelectionConfirmation, scan: bool) -> bool {
        match tap_check {
            TaskSelectionConfirmation::TapMarked { .. }
            | TaskSelectionConfirmation::PageChanged
            | TaskSelectionConfirmation::GuardFailed { .. }
            | TaskSelectionConfirmation::CaptureFailed { .. } => true,
            TaskSelectionConfirmation::MarkUnconfirmed { .. } => {
                self.mark_unconfirmed_allowed(tap_check, scan)
            }
            TaskSelectionConfirmation::PickMoved {
                pick_set_sha256, ..
            } => !scan && is_candidate_set_sha256(pick_set_sha256),
            TaskSelectionConfirmation::RepositionFailed { stage, .. } => {
                scan && *stage == TaskSelectionRepositionStage::TapCheck
            }
            TaskSelectionConfirmation::NotAttempted
            | TaskSelectionConfirmation::Matched { .. }
            | TaskSelectionConfirmation::Mismatched { .. }
            | TaskSelectionConfirmation::IdentityMatched { .. }
            | TaskSelectionConfirmation::IdentityUnconfirmed { .. }
            | TaskSelectionConfirmation::PickMatched { .. }
            | TaskSelectionConfirmation::PicksConfirmed { .. }
            | TaskSelectionConfirmation::PicksStopped { .. } => false,
        }
    }

    /// A `mark_unconfirmed` names a reading other than the one required, of an actionable
    /// candidate of the v1 projection (a mis-tapped neighbour included) or of a selected v2
    /// candidate.
    fn mark_unconfirmed_allowed(
        &self,
        confirmation: &TaskSelectionConfirmation,
        scan: bool,
    ) -> bool {
        let TaskSelectionConfirmation::MarkUnconfirmed {
            candidate_id,
            mark_expectation,
            mark_state,
            ..
        } = confirmation
        else {
            return false;
        };
        let named = if scan {
            self.selected.contains(candidate_id)
        } else {
            self.projection.evaluates(candidate_id)
        };
        named && !mark_expectation.met_by(*mark_state)
    }

    /// A single-frame select's `matched` or `mismatched`: its frame's hash against the v1
    /// projection's, and an embedded confirmation projection that seals that hash.
    fn validate_frame_comparison(&self) -> Result<(), SanitizationError> {
        let TaskSelectionProjection::V1(evaluated) = &self.projection else {
            return Err(invalid("confirmation"));
        };
        let evaluated = evaluated.candidate_set_sha256();
        let (candidate_set_sha256, projection, matched) = match &self.confirmation {
            TaskSelectionConfirmation::Matched {
                candidate_set_sha256,
                projection,
            } => (candidate_set_sha256, projection, true),
            TaskSelectionConfirmation::Mismatched {
                candidate_set_sha256,
                projection,
            } => (candidate_set_sha256, projection, false),
            _ => return Err(invalid("confirmation")),
        };
        if let Some(projection) = projection {
            projection.validate_decoded().map_err(projection_error)?;
            if projection.candidate_set_sha256() != candidate_set_sha256
                || projection.layout_id() != self.layout_id
                || projection.page_id() != self.page_id
            {
                return Err(invalid("confirmation"));
            }
        }
        let compares = if matched {
            candidate_set_sha256 == evaluated
        } else {
            is_candidate_set_sha256(candidate_set_sha256) && candidate_set_sha256 != evaluated
        };
        if !compares {
            return Err(invalid("confirmation"));
        }
        Ok(())
    }

    /// The `score_milli` of the one selected candidate's verdict; for a multi select, the
    /// lowest `score_milli` of the selected candidates' verdicts (M-357b M7).
    fn chosen_score(&self) -> Option<i64> {
        let score = |chosen: &String| {
            self.verdicts
                .iter()
                .find(|verdict| verdict.candidate_id == *chosen)
                .and_then(|verdict| verdict.score_milli)
        };
        if self.multi_select.is_none() {
            return score(self.selected.first()?);
        }
        let mut lowest = None;
        for chosen in &self.selected {
            let value = score(chosen)?;
            lowest = Some(lowest.map_or(value, |low: i64| low.max(value)));
        }
        lowest
    }
}

/// Where a multi select's picks stopped succeeding (M-357b M4).
enum PickEnd<'a> {
    /// (a) or (b): the first failing confirmation or tap check, which the record's top-level
    /// confirmation copies.
    Failed(&'a TaskSelectionConfirmation),
    /// (c) or (d): the sequence stopped outside the record at this pick, after as many picks
    /// were tapped and checked.
    Stopped(usize),
}

/// Whether an `identity_unconfirmed` keeps its members to its state: an observed identity
/// exactly when the state is `different`, and a reason for an Unknown reading only with
/// `unknown`.
fn identity_unconfirmed_consistent(
    state: TaskSelectionIdentityState,
    observed_identity: Option<&str>,
    identity_unknown: bool,
) -> bool {
    match state {
        TaskSelectionIdentityState::Absent => observed_identity.is_none() && !identity_unknown,
        TaskSelectionIdentityState::Unknown => observed_identity.is_none(),
        TaskSelectionIdentityState::Different => {
            !identity_unknown && observed_identity.is_some_and(is_business_identity)
        }
    }
}

impl TaskSelectionPolicy {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_task_semantic_label(&self.path, "policy_path")?;
        validate_policy_digest(&self.policy_sha256, "policy_sha256")?;
        validate_task_semantic_label(&self.policy_id, "policy_id")
    }
}

impl TaskSelectionOutcome {
    fn validate(&self) -> Result<(), SanitizationError> {
        match self {
            Self::Selected { count } if *count == 0 => Err(invalid("outcome")),
            Self::Insufficient {
                surviving,
                required,
            } if surviving >= required => Err(invalid("outcome")),
            Self::Ambiguous { candidate_ids }
                if candidate_ids.is_empty()
                    || candidate_ids.len() > TASK_SELECTION_MAX_CANDIDATES =>
            {
                Err(invalid("outcome"))
            }
            Self::Ambiguous { candidate_ids } => {
                for candidate_id in candidate_ids {
                    validate_task_semantic_label(candidate_id, "ambiguous_candidate_id")?;
                }
                Ok(())
            }
            Self::Unknown { .. }
            | Self::Selected { .. }
            | Self::Empty
            | Self::Insufficient { .. } => Ok(()),
        }
    }
}

impl TaskSelectionVerdict {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_task_semantic_label(&self.candidate_id, "verdict_candidate_id")?;
        for gate in &self.gates {
            validate_task_semantic_label(&gate.gate_id, "gate_id")?;
        }
        for term in &self.terms {
            validate_task_semantic_label(&term.term_id, "term_id")?;
        }
        validate_reasons(&self.reasons)
    }
}

fn validate_reasons(reasons: &[PolicyReasonRecord]) -> Result<(), SanitizationError> {
    for reason in reasons {
        validate_policy_token(reason.0.code().as_str(), "reason_code")?;
    }
    Ok(())
}

fn is_candidate_set_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn invalid(field: &'static str) -> SanitizationError {
    SanitizationError::new(INVALID, field)
}

fn projection_error(error: CandidateProjectionError) -> SanitizationError {
    SanitizationError::new(error.code(), error.item())
}

#[cfg(test)]
mod tests;
