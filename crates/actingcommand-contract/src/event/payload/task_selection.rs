// SPDX-License-Identifier: AGPL-3.0-only

// The record of one in-task selection (Workflow #308, `task.selection_evaluated`).
//
// A select step records exactly one of these per attempt, before any input and before any
// failure return: the candidate projection it evaluated, the identities of the policy and of
// the inputs, the full per-candidate breakdown, the choice, and how the confirmation frame
// compared. The breakdown types mirror the selection-policy evaluator's decision shapes field
// for field (`contracts/selection-graph.md`, section Records); this crate does not depend on
// the evaluator, and the kernel converts one into the other.
//
// A scan select (frozen model S0 v2.1 §1.9, slice SEL-L4a) records the scan-level projection v2,
// its own confirmation variants and `unknown_check`, and the record is checked against the
// path table of S0 §3.4 through the seven path rules of §1.9.

use super::{
    PolicyReasonRecord, validate_policy_digest, validate_policy_token, validate_task_semantic_label,
};
use crate::candidate_projection::is_business_identity;
use crate::outcome::Outcome;
use crate::{
    CANDIDATE_PROJECTION_SCHEMA_VERSION, CandidateProjection, CandidateProjectionError,
    CandidateUnknownReason, SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION, SanitizationError,
    ScanCandidateProjection, ScanCoverageState,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::collections::BTreeSet;

/// Candidates one record may break down; the projection's own candidate budget.
pub const TASK_SELECTION_MAX_CANDIDATES: usize = crate::CANDIDATE_PROJECTION_MAX_CANDIDATES;
/// Compact JSON bytes of one record.
pub const TASK_SELECTION_RECORD_MAX_BYTES: usize = 64 * 1024;
/// Entries of the decision-level reason chain, as for a policy dispatch.
pub const TASK_SELECTION_MAX_REASONS: usize = 128;
/// The record's compact JSON exceeds [`TASK_SELECTION_RECORD_MAX_BYTES`].
pub const SELECTION_RECORD_TOO_LARGE: crate::outcome::Code =
    crate::codes::SanitizationCode::SelectionRecordTooLarge.code();

const INVALID: crate::outcome::Code =
    crate::codes::SanitizationCode::InvalidTaskSelectionRecord.code();

/// The selection policy the step evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionPolicy {
    /// The package-relative document path the step declares.
    pub path: String,
    /// SHA-256 of the document's bytes in the package: the step's declared `sha256`.
    pub package_sha256: String,
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

/// Where a scan select's repositioning failed: returning to the target, or aligning F2 against
/// the content map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSelectionRepositionStage {
    Return,
    Confirm,
}

/// How the confirmation frame compared with the evaluated projection.
///
/// A confirmation is attempted exactly when the outcome is `selected` and nothing blocks it
/// (path rule 7). `matched` and `mismatched` carry the confirmation frame's candidate-set hash
/// and belong to a single-frame select (v1); `identity_matched`, `identity_unconfirmed` and
/// `reposition_failed` belong to a scan select (v2).
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
    /// the failure travels as `outcome`, as for `guard_failed` (ledger revision 3).
    RepositionFailed {
        outcome: Outcome,
        stage: TaskSelectionRepositionStage,
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
    pub input_ledger_position: u64,
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
        if self.input_ledger_position == 0 {
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
        self.validate_paths()
    }

    /// [`Self::validate`] plus the byte budgets, measured on the current encoding: the
    /// projection within its own budget and the whole record within
    /// [`TASK_SELECTION_RECORD_MAX_BYTES`] (`selection_record_too_large`). Write side only;
    /// the producer runs it before any input.
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
    /// `confirmation`, every other rule names itself.
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
        // A select chooses exactly one candidate, whose verdict carries the chosen score.
        if selected && self.selected.len() != 1 {
            return Err(invalid("selected"));
        }

        // Rule 1: R unknown ⇔ C not_started or incomplete; R empty, insufficient or ambiguous ⇒
        // C complete; R selected ⇒ C complete or early_stopped; C early_stopped ⇒ R selected and
        // B none.
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
            if !holds {
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

        // Rule 4: B unknown_instances ⇔ R empty or insufficient with an unknown instance.
        let unread_blocks = matches!(
            result,
            TaskSelectionOutcome::Empty | TaskSelectionOutcome::Insufficient { .. }
        ) && unknown_instances >= 1;
        if (check.blocking == TaskSelectionBlocking::UnknownInstances) != unread_blocks {
            return Err(invalid("path_rule_4"));
        }

        // Rule 5: B outranked_possible ⇔ R selected and the bound is unbounded, or a maximum
        // above the chosen score.
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
    /// `identity_matched` names the selected candidate.
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
                let consistent = match identity_state {
                    TaskSelectionIdentityState::Absent => {
                        observed_identity.is_none() && identity_unknown.is_none()
                    }
                    TaskSelectionIdentityState::Unknown => observed_identity.is_none(),
                    TaskSelectionIdentityState::Different => {
                        identity_unknown.is_none()
                            && observed_identity
                                .as_deref()
                                .is_some_and(is_business_identity)
                    }
                };
                if !scan || !consistent {
                    return Err(invalid("confirmation"));
                }
                Ok(())
            }
            TaskSelectionConfirmation::RepositionFailed { .. } => {
                if !scan {
                    return Err(invalid("confirmation"));
                }
                Ok(())
            }
        }
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

    /// The `score_milli` of the one selected candidate's verdict.
    fn chosen_score(&self) -> Option<i64> {
        let chosen = self.selected.first()?;
        self.verdicts
            .iter()
            .find(|verdict| verdict.candidate_id == *chosen)
            .and_then(|verdict| verdict.score_milli)
    }
}

impl TaskSelectionPolicy {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_task_semantic_label(&self.path, "policy_path")?;
        if !is_candidate_set_sha256(&self.package_sha256) {
            return Err(invalid("policy_package_sha256"));
        }
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
