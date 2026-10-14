// SPDX-License-Identifier: AGPL-3.0-only

//! `actingcommand.candidate-projection.v2`: the projection of one scan select (Workflow #308,
//! frozen model S0 v2.1 §1.1 and §1.9; slice SEL-L4a).
//!
//! A scan select reads a list over several viewports and records the whole scan in content
//! coordinates: the coverage it reached, its coverage viewports, how often it repositioned, the
//! member candidates it fixed, the positions it could not read and how many positions a gate
//! excluded. v1 stays the projection of a single-frame select and of observation; only the scan
//! session produces v2. This module owns the shape, the reader bounds, the validation and the
//! hash. Producing a v2 projection belongs to the scan session (SEL-L3, SEL-L4b).

use super::{
    CANDIDATE_PROJECTION_MAX_BYTES, CANDIDATE_PROJECTION_MAX_CANDIDATES, CandidateFeatureMap,
    CandidateFrame, CandidateLayoutKind, CandidateProjectionError, CandidateRect,
    CandidateUnknownReason, HashedFeature, hashed_features, is_business_identity,
    is_lower_hex_sha256, validate_candidate_features, validate_candidate_layout_id,
    validate_page_id,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION: &str = "actingcommand.candidate-projection.v2";

/// Coverage viewports one scan projection holds: a reader bound, frozen with ledger revision 3.
/// The writer's own bounds (`max_viewports` at admission, the byte budget) are lower.
pub const SCAN_PROJECTION_MAX_VIEWPORTS: usize = 256;
/// Member candidates one scan projection holds (reader bound); IDs stay `{layout_id}#{NN}`.
pub const SCAN_PROJECTION_MAX_CANDIDATES: usize = CANDIDATE_PROJECTION_MAX_CANDIDATES;
/// Unknown positions one scan projection holds (reader bound).
pub const SCAN_PROJECTION_MAX_UNKNOWN: usize = 64;

/// The axis the list moves along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanAxis {
    Vertical,
    Horizontal,
}

/// How far the scan got. `not_started`: the decision was taken at `Pin`, before any viewport;
/// `incomplete`: the evaluator aborted mid-scan; `early_stopped`: the early-stop test held;
/// `complete`: the start and end checks both passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanCoverageState {
    NotStarted,
    Incomplete,
    EarlyStopped,
    Complete,
}

/// How the start of the list was confirmed: on the first settled frame, or after a rewind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanStartEvidence {
    StartCheck,
    RewindCheck,
}

/// How the end of the list was confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanEndEvidence {
    EndCheck,
}

/// The coverage the scan reached. The state decides the members: `not_started` has none,
/// `incomplete` only `start`, `early_stopped` `start` and `extent_px`, `complete` all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanCoverage {
    pub state: ScanCoverageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ScanStartEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<ScanEndEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent_px: Option<u32>,
}

/// What brought a coverage viewport: the start of the scan, a forward advance, or the one
/// corrective reverse advance that closes a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanViewportMotion {
    Start,
    Forward,
    Corrective,
}

/// One coverage viewport. Viewport 0 is the start at content offset 0; `frame_id` links the
/// viewport frame's own capture and recognition records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanViewport {
    pub motion: ScanViewportMotion,
    pub content_offset_px: i32,
    pub tracked_instances: u32,
    pub readable_instances: u32,
    pub unknown_instances: u32,
    pub frame_id: crate::FrameId,
}

/// The rewind and return viewports, counted only; they are not coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanReposition {
    pub rewind_viewports: u32,
    pub return_viewports: u32,
}

/// One member candidate, numbered in first-seen order. Every member was actionable when its
/// reading was fixed; `fixed_viewport` is the coverage viewport whose reading fixed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanCandidate {
    pub id: String,
    pub instance_index: u32,
    pub actionable: bool,
    pub content_rect: CandidateRect,
    pub fixed_viewport: u32,
    pub features: CandidateFeatureMap,
}

/// Why a read position has no fixed Known reading; when several apply, the first in this order
/// is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanUnknownCause {
    /// No Known identity was ever read.
    IdentityUnread,
    /// The identity was Known, but no reading was complete.
    FeatureUnread,
    /// A later Known reading of a policy-read feature differed.
    Contradicted,
    /// The evaluator's verdict was `unknown_dropped`.
    UnknownDropped,
}

/// One unknown position. `identity` is the Known business ID the position kept, present
/// exactly when the cause is not `identity_unread`; `identity_unknown` may then say why the
/// identity was Unknown. `first_read_viewport` is the first coverage viewport in which the
/// position was actionable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanUnknownPosition {
    pub position_index: u32,
    pub content_rect: CandidateRect,
    pub first_read_viewport: u32,
    pub cause: ScanUnknownCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_unknown: Option<CandidateUnknownReason>,
}

/// The positions a gate excluded: their count and the SHA-256 of their identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanGatedOut {
    pub count: u32,
    pub identities_sha256: String,
}

/// What a scan session supplies; [`ScanCandidateProjection::new`] adds the schema version and
/// the layout kind and seals the candidate-set hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanCandidateProjectionParts {
    pub page_id: String,
    pub layout_id: String,
    pub axis: ScanAxis,
    pub frame: CandidateFrame,
    pub coverage: ScanCoverage,
    pub viewports: Vec<ScanViewport>,
    pub reposition: ScanReposition,
    pub candidates: Vec<ScanCandidate>,
    pub unknown: Vec<ScanUnknownPosition>,
    pub gated_out: ScanGatedOut,
}

/// The scan-level projection, in content coordinates, with its sealed hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanCandidateProjection {
    schema_version: String,
    page_id: String,
    layout_id: String,
    layout_kind: CandidateLayoutKind,
    axis: ScanAxis,
    frame: CandidateFrame,
    coverage: ScanCoverage,
    viewports: Vec<ScanViewport>,
    reposition: ScanReposition,
    candidates: Vec<ScanCandidate>,
    unknown: Vec<ScanUnknownPosition>,
    gated_out: ScanGatedOut,
    candidate_set_sha256: String,
}

impl ScanCandidateProjection {
    /// Checks the parts, seals the candidate-set hash and checks the byte budget.
    pub fn new(parts: ScanCandidateProjectionParts) -> Result<Self, CandidateProjectionError> {
        let mut projection = Self {
            schema_version: SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION.to_owned(),
            page_id: parts.page_id,
            layout_id: parts.layout_id,
            layout_kind: CandidateLayoutKind::RepeatedAnchor,
            axis: parts.axis,
            frame: parts.frame,
            coverage: parts.coverage,
            viewports: parts.viewports,
            reposition: parts.reposition,
            candidates: parts.candidates,
            unknown: parts.unknown,
            gated_out: parts.gated_out,
            candidate_set_sha256: String::new(),
        };
        projection.validate_shape()?;
        projection.candidate_set_sha256 = projection.compute_candidate_set_sha256()?;
        projection.validate_encoded_size()?;
        Ok(projection)
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    pub fn page_id(&self) -> &str {
        &self.page_id
    }

    pub fn layout_id(&self) -> &str {
        &self.layout_id
    }

    pub const fn layout_kind(&self) -> CandidateLayoutKind {
        self.layout_kind
    }

    pub const fn axis(&self) -> ScanAxis {
        self.axis
    }

    pub const fn frame(&self) -> CandidateFrame {
        self.frame
    }

    pub const fn coverage(&self) -> ScanCoverage {
        self.coverage
    }

    pub fn viewports(&self) -> &[ScanViewport] {
        &self.viewports
    }

    pub const fn reposition(&self) -> ScanReposition {
        self.reposition
    }

    pub fn candidates(&self) -> &[ScanCandidate] {
        &self.candidates
    }

    pub fn unknown(&self) -> &[ScanUnknownPosition] {
        &self.unknown
    }

    pub const fn gated_out(&self) -> &ScanGatedOut {
        &self.gated_out
    }

    pub fn candidate_set_sha256(&self) -> &str {
        &self.candidate_set_sha256
    }

    pub fn candidate(&self, id: &str) -> Option<&ScanCandidate> {
        self.candidates.iter().find(|candidate| candidate.id == id)
    }

    /// Checks the shape, that the sealed hash equals the recomputed one, and the byte budget.
    pub fn validate(&self) -> Result<(), CandidateProjectionError> {
        self.validate_decoded()?;
        self.validate_encoded_size()
    }

    /// Shape, reader bounds and sealed hash only; a decoded record is never re-measured for a
    /// byte budget.
    pub(crate) fn validate_decoded(&self) -> Result<(), CandidateProjectionError> {
        self.validate_shape()?;
        if !is_lower_hex_sha256(&self.candidate_set_sha256) {
            return Err(CandidateProjectionError::invalid(
                "candidate_set_sha256",
                "the candidate-set hash is not 64 lowercase hexadecimal digits",
            ));
        }
        if self.compute_candidate_set_sha256()? != self.candidate_set_sha256 {
            return Err(CandidateProjectionError::invalid(
                "candidate_set_sha256",
                "the sealed candidate-set hash differs from the candidates",
            ));
        }
        Ok(())
    }

    /// The writer's byte budget: the compact JSON within [`CANDIDATE_PROJECTION_MAX_BYTES`].
    pub fn validate_encoded_size(&self) -> Result<(), CandidateProjectionError> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            CandidateProjectionError::invalid("projection", format!("encoding failed: {error}"))
        })?;
        if bytes.len() > CANDIDATE_PROJECTION_MAX_BYTES {
            return Err(CandidateProjectionError::budget_exceeded(
                "projection_bytes",
                format!(
                    "{} bytes exceed the {CANDIDATE_PROJECTION_MAX_BYTES}-byte projection budget",
                    bytes.len()
                ),
            ));
        }
        Ok(())
    }

    /// SHA-256, as 64 lowercase hexadecimal digits, of the compact JSON of `schema_version`,
    /// `page_id`, `layout_id`, `layout_kind`, `axis`, `frame`, `coverage` with only `state`,
    /// `candidates`, `unknown` and `gated_out`, in exactly this order. Features enter as in v1:
    /// type and value only. The members that describe the path the scan took stay out, so equal
    /// lists scanned differently hash alike: `viewports`, `reposition`, the other `coverage`
    /// members, and the record-only viewport indices `fixed_viewport` and `first_read_viewport`.
    pub fn compute_candidate_set_sha256(&self) -> Result<String, CandidateProjectionError> {
        let view = HashedScanProjection {
            schema_version: &self.schema_version,
            page_id: &self.page_id,
            layout_id: &self.layout_id,
            layout_kind: self.layout_kind,
            axis: self.axis,
            frame: self.frame,
            coverage: HashedCoverage {
                state: self.coverage.state,
            },
            candidates: self
                .candidates
                .iter()
                .map(|candidate| HashedScanCandidate {
                    id: &candidate.id,
                    instance_index: candidate.instance_index,
                    actionable: candidate.actionable,
                    content_rect: candidate.content_rect,
                    features: hashed_features(&candidate.features),
                })
                .collect(),
            unknown: self
                .unknown
                .iter()
                .map(|position| HashedScanUnknown {
                    position_index: position.position_index,
                    content_rect: position.content_rect,
                    cause: position.cause,
                    identity: position.identity.as_deref(),
                    identity_unknown: position.identity_unknown,
                })
                .collect(),
            gated_out: &self.gated_out,
        };
        let bytes = serde_json::to_vec(&view).map_err(|error| {
            CandidateProjectionError::invalid(
                "candidate_set_sha256",
                format!("hash encoding failed: {error}"),
            )
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn validate_shape(&self) -> Result<(), CandidateProjectionError> {
        if self.schema_version != SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION {
            return Err(CandidateProjectionError::invalid(
                "schema_version",
                format!(
                    "`{}` is not `{SCAN_CANDIDATE_PROJECTION_SCHEMA_VERSION}`",
                    self.schema_version
                ),
            ));
        }
        validate_page_id(&self.page_id)?;
        validate_candidate_layout_id(&self.layout_id)?;
        if self.layout_kind != CandidateLayoutKind::RepeatedAnchor {
            return Err(CandidateProjectionError::invalid(
                "layout_kind",
                "a scan projection is taken over a repeated_anchor layout",
            ));
        }
        if self.frame.width == 0 || self.frame.height == 0 {
            return Err(CandidateProjectionError::invalid(
                "frame",
                "the frame has a zero dimension",
            ));
        }
        for (item, count, bound) in [
            (
                "scan_viewports",
                self.viewports.len(),
                SCAN_PROJECTION_MAX_VIEWPORTS,
            ),
            (
                "scan_candidates",
                self.candidates.len(),
                SCAN_PROJECTION_MAX_CANDIDATES,
            ),
            (
                "scan_unknown",
                self.unknown.len(),
                SCAN_PROJECTION_MAX_UNKNOWN,
            ),
        ] {
            if count > bound {
                return Err(CandidateProjectionError::budget_exceeded(
                    item,
                    format!("{count} entries exceed the {bound}-entry reader bound"),
                ));
            }
        }
        self.validate_coverage()?;
        let viewports = self.viewports.len();
        for (index, candidate) in self.candidates.iter().enumerate() {
            let expected = format!("{}#{index:02}", self.layout_id);
            if candidate.instance_index as usize != index {
                return Err(CandidateProjectionError::invalid(
                    "instance_index",
                    format!(
                        "member at position {index} carries instance index {}",
                        candidate.instance_index
                    ),
                ));
            }
            if candidate.id != expected {
                return Err(CandidateProjectionError::invalid(
                    "candidate_id",
                    format!("member `{}` is not `{expected}`", candidate.id),
                ));
            }
            if !candidate.actionable {
                return Err(CandidateProjectionError::invalid(
                    "actionable",
                    format!("member `{expected}` is not actionable"),
                ));
            }
            if !candidate.content_rect.is_valid() {
                return Err(CandidateProjectionError::invalid(
                    "rect",
                    format!("member `{expected}` has an empty or overflowing content rectangle"),
                ));
            }
            if candidate.fixed_viewport as usize >= viewports {
                return Err(CandidateProjectionError::invalid(
                    "fixed_viewport",
                    format!("member `{expected}` names a viewport the scan does not hold"),
                ));
            }
            validate_candidate_features(&expected, true, &candidate.features)?;
        }
        let mut positions = BTreeSet::new();
        for position in &self.unknown {
            if !positions.insert(position.position_index) {
                return Err(CandidateProjectionError::invalid(
                    "position_index",
                    format!(
                        "unknown position {} is listed twice",
                        position.position_index
                    ),
                ));
            }
            if !position.content_rect.is_valid() {
                return Err(CandidateProjectionError::invalid(
                    "rect",
                    format!(
                        "unknown position {} has an empty or overflowing content rectangle",
                        position.position_index
                    ),
                ));
            }
            if position.first_read_viewport as usize >= viewports {
                return Err(CandidateProjectionError::invalid(
                    "first_read_viewport",
                    format!(
                        "unknown position {} names a viewport the scan does not hold",
                        position.position_index
                    ),
                ));
            }
            let unread = position.cause == ScanUnknownCause::IdentityUnread;
            let identity_valid = match &position.identity {
                None => unread,
                Some(identity) => {
                    !unread && position.identity_unknown.is_none() && is_business_identity(identity)
                }
            };
            if !identity_valid {
                return Err(CandidateProjectionError::invalid(
                    "unknown_identity",
                    format!(
                        "unknown position {} keeps an identity exactly when its cause is not identity_unread",
                        position.position_index
                    ),
                ));
            }
        }
        if !is_lower_hex_sha256(&self.gated_out.identities_sha256) {
            return Err(CandidateProjectionError::invalid(
                "gated_out",
                "the gated-out identity hash is not 64 lowercase hexadecimal digits",
            ));
        }
        Ok(())
    }

    /// The coverage members by state (S0 §1.9), and path rule 2: a scan that never started has
    /// no viewport, member or unknown position; any other begins with its start viewport at
    /// content offset 0, and no later viewport is a start.
    fn validate_coverage(&self) -> Result<(), CandidateProjectionError> {
        let coverage = self.coverage;
        let (start, end, extent) = (
            coverage.start.is_some(),
            coverage.end.is_some(),
            coverage.extent_px.is_some(),
        );
        let members = match coverage.state {
            ScanCoverageState::NotStarted => !start && !end && !extent,
            ScanCoverageState::Incomplete => start && !end && !extent,
            ScanCoverageState::EarlyStopped => start && !end && extent,
            ScanCoverageState::Complete => start && end && extent,
        };
        if !members {
            return Err(CandidateProjectionError::invalid(
                "coverage",
                "the coverage members do not match its state",
            ));
        }
        let path = if coverage.state == ScanCoverageState::NotStarted {
            self.viewports.is_empty() && self.candidates.is_empty() && self.unknown.is_empty()
        } else {
            self.viewports.first().is_some_and(|viewport| {
                viewport.motion == ScanViewportMotion::Start && viewport.content_offset_px == 0
            }) && self
                .viewports
                .iter()
                .skip(1)
                .all(|viewport| viewport.motion != ScanViewportMotion::Start)
        };
        if !path {
            return Err(CandidateProjectionError::invalid(
                "viewports",
                "a started scan begins with its start viewport at offset 0; one never started holds no viewport, member or unknown position",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct HashedScanProjection<'a> {
    schema_version: &'a str,
    page_id: &'a str,
    layout_id: &'a str,
    layout_kind: CandidateLayoutKind,
    axis: ScanAxis,
    frame: CandidateFrame,
    coverage: HashedCoverage,
    candidates: Vec<HashedScanCandidate<'a>>,
    unknown: Vec<HashedScanUnknown<'a>>,
    gated_out: &'a ScanGatedOut,
}

#[derive(Serialize)]
struct HashedCoverage {
    state: ScanCoverageState,
}

#[derive(Serialize)]
struct HashedScanCandidate<'a> {
    id: &'a str,
    instance_index: u32,
    actionable: bool,
    content_rect: CandidateRect,
    features: BTreeMap<&'a str, HashedFeature<'a>>,
}

#[derive(Serialize)]
struct HashedScanUnknown<'a> {
    position_index: u32,
    content_rect: CandidateRect,
    cause: ScanUnknownCause,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity_unknown: Option<CandidateUnknownReason>,
}
