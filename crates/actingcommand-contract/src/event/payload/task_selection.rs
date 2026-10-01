// SPDX-License-Identifier: AGPL-3.0-only

// The record of one in-task selection (Workflow #308, `task.selection_evaluated`).
//
// A select step records exactly one of these per attempt, before any input and before any
// failure return: the candidate projection it evaluated, the identities of the policy and of
// the inputs, the full per-candidate breakdown, the choice, and how the confirmation frame
// compared. The breakdown types mirror the selection-policy evaluator's decision shapes field
// for field (`contracts/selection-graph.md`, section Records); this crate does not depend on
// the evaluator, and the kernel converts one into the other.

use super::{
    PolicyReasonRecord, validate_policy_digest, validate_policy_text, validate_policy_token,
    validate_task_semantic_label,
};
use crate::{CandidateProjection, CandidateProjectionError, SanitizationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Candidates one record may break down; the projection's own candidate budget.
pub const TASK_SELECTION_MAX_CANDIDATES: usize = crate::CANDIDATE_PROJECTION_MAX_CANDIDATES;
/// Compact JSON bytes of one record.
pub const TASK_SELECTION_RECORD_MAX_BYTES: usize = 64 * 1024;
/// Entries of the decision-level reason chain, as for a policy dispatch.
pub const TASK_SELECTION_MAX_REASONS: usize = 128;
/// The record's compact JSON exceeds [`TASK_SELECTION_RECORD_MAX_BYTES`].
pub const SELECTION_RECORD_TOO_LARGE: &str = "selection_record_too_large";

const INVALID: &str = "invalid_task_selection_record";

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

/// Mirrors the evaluator's `SelectionOutcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionOutcome {
    Selected {
        count: u32,
    },
    Empty,
    Insufficient {
        surviving: u32,
        required: u32,
    },
    Ambiguous {
        candidate_ids: Vec<String>,
    },
    Unknown {
        reason: TaskSelectionUnknownReason,
        detail: String,
    },
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
    pub outcome: TaskSelectionGateOutcome,
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
    pub outcome: TaskSelectionTermOutcome,
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

/// Mirrors the evaluator's `CandidateVerdict`; its reasons use the policy reason record,
/// which has the evaluator's `DecisionReason` shape.
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

/// How the confirmation frame compared with the evaluated projection.
///
/// A confirmation is attempted exactly when the outcome is `selected`. `matched` and
/// `mismatched` carry the confirmation frame's candidate-set hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskSelectionConfirmation {
    NotAttempted,
    Matched { candidate_set_sha256: String },
    Mismatched { candidate_set_sha256: String },
    PageChanged,
    GuardFailed { code: String },
    CaptureFailed { code: String },
}

/// One select step's decision record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSelectionRecord {
    pub layout_id: String,
    pub page_id: String,
    /// The core projection of the step's frame, whole.
    pub projection: CandidateProjection,
    pub policy: TaskSelectionPolicy,
    pub fact_snapshot_id: String,
    pub input_ledger_position: u64,
    pub now_unix_ms: u64,
    /// The evaluator's canonical identity of candidates, facts and instant, `sha256:<hex>`.
    pub input_sha256: String,
    pub outcome: TaskSelectionOutcome,
    pub outcome_key: String,
    pub selected: Vec<String>,
    pub verdicts: Vec<TaskSelectionVerdict>,
    pub reasons: Vec<PolicyReasonRecord>,
    pub confirmation: TaskSelectionConfirmation,
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
        self.outcome.validate()?;
        validate_task_semantic_label(&self.outcome_key, "outcome_key")?;

        if self.verdicts.len() > TASK_SELECTION_MAX_CANDIDATES {
            return Err(invalid("verdicts"));
        }
        let mut evaluated = BTreeSet::new();
        for verdict in &self.verdicts {
            verdict.validate()?;
            let actionable = self
                .projection
                .candidate(&verdict.candidate_id)
                .is_some_and(|candidate| candidate.actionable);
            if !actionable || !evaluated.insert(verdict.candidate_id.as_str()) {
                return Err(invalid("verdicts"));
            }
        }

        let expected = match self.outcome {
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
        self.validate_confirmation()
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
        let bytes = serde_json::to_vec(self).map_err(|_| invalid("selection"))?;
        if bytes.len() > TASK_SELECTION_RECORD_MAX_BYTES {
            return Err(SanitizationError::new(
                SELECTION_RECORD_TOO_LARGE,
                "selection",
            ));
        }
        Ok(())
    }

    fn validate_confirmation(&self) -> Result<(), SanitizationError> {
        let selected = matches!(self.outcome, TaskSelectionOutcome::Selected { .. });
        let attempted = !matches!(self.confirmation, TaskSelectionConfirmation::NotAttempted);
        if selected != attempted {
            return Err(invalid("confirmation"));
        }
        let evaluated = self.projection.candidate_set_sha256();
        match &self.confirmation {
            TaskSelectionConfirmation::NotAttempted | TaskSelectionConfirmation::PageChanged => {}
            TaskSelectionConfirmation::Matched {
                candidate_set_sha256,
            } => {
                if candidate_set_sha256 != evaluated {
                    return Err(invalid("confirmation"));
                }
            }
            TaskSelectionConfirmation::Mismatched {
                candidate_set_sha256,
            } => {
                if !is_candidate_set_sha256(candidate_set_sha256)
                    || candidate_set_sha256 == evaluated
                {
                    return Err(invalid("confirmation"));
                }
            }
            TaskSelectionConfirmation::GuardFailed { code }
            | TaskSelectionConfirmation::CaptureFailed { code } => {
                validate_policy_token(code, "confirmation_code")?;
            }
        }
        Ok(())
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
            Self::Unknown { detail, .. } => validate_policy_text(detail, "outcome_detail"),
            Self::Selected { .. } | Self::Empty | Self::Insufficient { .. } => Ok(()),
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
        validate_policy_token(&reason.code, "reason_code")?;
        validate_policy_text(&reason.detail, "reason_detail")?;
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
