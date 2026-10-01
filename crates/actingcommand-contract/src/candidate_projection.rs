// SPDX-License-Identifier: AGPL-3.0-only

//! `actingcommand.candidate-projection.v1`: the candidate set one declared candidate layout
//! yields on one frame (Workflow #308, `contracts/candidate-projection.md`).
//!
//! The core projection is what the selection evaluator consumes and what the task ledger
//! records. Its `candidate_set_sha256` covers every candidate's geometry and every feature's
//! type and value, and never a feature's `confidence` or the frame's identity, so two frames of
//! an unchanged screen hash alike. The public form withholds the features of a personal
//! candidate; those features travel only in the controlled evidence map. This module owns the
//! shapes, the budgets, the identifier grammar and the hash. Producing a projection from a
//! scene belongs to the recognition pack.

use crate::page_projection::Privacy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

pub const CANDIDATE_PROJECTION_SCHEMA_VERSION: &str = "actingcommand.candidate-projection.v1";

/// Candidates one layout yields on one frame.
pub const CANDIDATE_PROJECTION_MAX_CANDIDATES: usize = 64;
/// Features one layout declares, and so features one candidate carries.
pub const CANDIDATE_PROJECTION_MAX_FEATURES: usize = 8;
/// Layouts one page declares.
pub const CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE: usize = 4;
/// Layouts one package declares.
pub const CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE: usize = 64;
/// OCR and NN feature evaluations one projection performs.
pub const CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS: usize = 16;
/// Compact JSON bytes of one core projection.
pub const CANDIDATE_PROJECTION_MAX_BYTES: usize = 32 * 1024;
/// Compact JSON bytes of every public candidate set one observation carries.
pub const CANDIDATE_SETS_MAX_BYTES: usize = 32 * 1024;

/// Any per-projection budget above; the error names the exceeded item.
pub const CANDIDATE_PROJECTION_BUDGET_EXCEEDED: &str = "candidate_projection_budget_exceeded";
/// The public candidate sets of one observation exceed [`CANDIDATE_SETS_MAX_BYTES`].
pub const CANDIDATE_SETS_BUDGET_EXCEEDED: &str = "candidate_sets_budget_exceeded";
/// A projection, a public candidate set or a privacy split breaks this contract's shape.
pub const INVALID_CANDIDATE_PROJECTION: &str = "invalid_candidate_projection";
/// A candidate identifier is not `{layout_id}#{NN}`.
pub const INVALID_CANDIDATE_ID: &str = "invalid_candidate_id";

pub const CANDIDATE_LAYOUT_ID_MAX_BYTES: usize = 64;
pub const CANDIDATE_FEATURE_NAME_MAX_BYTES: usize = 32;
const PAGE_ID_MAX_BYTES: usize = 256;

/// One failure of this contract: its code, the item or field it concerns, and a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateProjectionError {
    code: &'static str,
    item: &'static str,
    detail: String,
}

impl CandidateProjectionError {
    /// A per-projection budget item was exceeded; nothing is truncated.
    pub fn budget_exceeded(item: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code: CANDIDATE_PROJECTION_BUDGET_EXCEEDED,
            item,
            detail: detail.into(),
        }
    }

    fn invalid(item: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code: INVALID_CANDIDATE_PROJECTION,
            item,
            detail: detail.into(),
        }
    }

    fn invalid_id(detail: impl Into<String>) -> Self {
        Self {
            code: INVALID_CANDIDATE_ID,
            item: "candidate_id",
            detail: detail.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// The budget item or field the failure concerns, e.g. `candidates` or `projection_bytes`.
    pub const fn item(&self) -> &'static str {
        self.item
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for CandidateProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}: {}", self.code, self.item, self.detail)
    }
}

impl std::error::Error for CandidateProjectionError {}

/// How the layout places its candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateLayoutKind {
    FixedSlots,
    RepeatedAnchor,
}

/// The dimensions of the frame the projection was taken on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateFrame {
    pub width: u32,
    pub height: u32,
}

/// A rectangle in frame pixels. An instance at the frame edge may reach outside the frame, so
/// the origin is signed; such an instance is never actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl CandidateRect {
    fn is_valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.x.checked_add(self.width).is_some()
            && self.y.checked_add(self.height).is_some()
    }

    /// Whether the rectangle lies entirely inside a frame of this size.
    pub fn is_within(self, frame: CandidateFrame) -> bool {
        self.is_valid()
            && self.x >= 0
            && self.y >= 0
            && i64::from(self.x) + i64::from(self.width) <= i64::from(frame.width)
            && i64::from(self.y) + i64::from(self.height) <= i64::from(frame.height)
    }
}

/// One feature value of one candidate.
///
/// `passed` features are booleans and `measure_milli` features are integers. `confidence` is
/// the backend's own confidence in integer milli, or `null` when the backend has none; it is
/// not part of the candidate-set hash and v1 does not hand it to the evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateFeature {
    Integer {
        value: i64,
        confidence: Option<i64>,
    },
    Boolean {
        value: bool,
        confidence: Option<i64>,
    },
}

/// Features by declared name; the map keeps them sorted by name.
pub type CandidateFeatureMap = BTreeMap<String, CandidateFeature>;

/// The controlled features of personal candidates, by candidate ID. Evidence only.
pub type PrivateCandidateFeatures = BTreeMap<String, CandidateFeatureMap>;

/// One candidate of the core projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedCandidate {
    pub id: String,
    pub instance_index: u32,
    pub actionable: bool,
    pub rect: CandidateRect,
    pub click: CandidateRect,
    pub features: CandidateFeatureMap,
}

/// The core candidate set: every feature of every candidate, with its sealed hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateProjection {
    schema_version: String,
    page_id: String,
    layout_id: String,
    layout_kind: CandidateLayoutKind,
    frame: CandidateFrame,
    candidates: Vec<ProjectedCandidate>,
    candidate_set_sha256: String,
}

impl CandidateProjection {
    /// Checks the candidates, seals the candidate-set hash and checks the byte budget.
    pub fn new(
        page_id: impl Into<String>,
        layout_id: impl Into<String>,
        layout_kind: CandidateLayoutKind,
        frame: CandidateFrame,
        candidates: Vec<ProjectedCandidate>,
    ) -> Result<Self, CandidateProjectionError> {
        let mut projection = Self {
            schema_version: CANDIDATE_PROJECTION_SCHEMA_VERSION.to_owned(),
            page_id: page_id.into(),
            layout_id: layout_id.into(),
            layout_kind,
            frame,
            candidates,
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

    pub const fn frame(&self) -> CandidateFrame {
        self.frame
    }

    pub fn candidates(&self) -> &[ProjectedCandidate] {
        &self.candidates
    }

    pub fn candidate_set_sha256(&self) -> &str {
        &self.candidate_set_sha256
    }

    pub fn candidate(&self, id: &str) -> Option<&ProjectedCandidate> {
        self.candidates.iter().find(|candidate| candidate.id == id)
    }

    /// Checks the shape, that the sealed hash equals the recomputed one, and the byte budget.
    pub fn validate(&self) -> Result<(), CandidateProjectionError> {
        self.validate_decoded()?;
        self.validate_encoded_size()
    }

    /// Shape and sealed hash only; a decoded record is never re-measured for a byte budget.
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

    /// The compact JSON of the core projection within [`CANDIDATE_PROJECTION_MAX_BYTES`].
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

    /// SHA-256, as 64 lowercase hexadecimal digits, of the compact JSON of the set header
    /// (`schema_version`, `page_id`, `layout_id`, `layout_kind`, `frame` with `width` and
    /// `height`) followed by `candidates`, each with `id`, `instance_index`, `actionable`,
    /// `rect`, `click` and `features` (by name, sorted, each only `type` and `value`), in
    /// exactly this field order. `confidence` and the frame's identity are not covered.
    pub fn compute_candidate_set_sha256(&self) -> Result<String, CandidateProjectionError> {
        let view = HashedProjection {
            schema_version: &self.schema_version,
            page_id: &self.page_id,
            layout_id: &self.layout_id,
            layout_kind: self.layout_kind,
            frame: self.frame,
            candidates: self
                .candidates
                .iter()
                .map(|candidate| HashedCandidate {
                    id: &candidate.id,
                    instance_index: candidate.instance_index,
                    actionable: candidate.actionable,
                    rect: candidate.rect,
                    click: candidate.click,
                    features: candidate
                        .features
                        .iter()
                        .map(|(name, feature)| (name.as_str(), HashedFeature::from(*feature)))
                        .collect(),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&view).map_err(|error| {
            CandidateProjectionError::invalid(
                "candidate_set_sha256",
                format!("hash encoding failed: {error}"),
            )
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Splits the projection into its public form and the controlled features of its personal
    /// candidates. `privacy` holds one entry per candidate, in candidate order: the strictest
    /// privacy of the targets the candidate's features read. Set-level fields, the hash
    /// included, are kept as they are; nothing is rehashed.
    pub fn split(
        &self,
        privacy: &[Privacy],
    ) -> Result<(PublicCandidateSet, PrivateCandidateFeatures), CandidateProjectionError> {
        if privacy.len() != self.candidates.len() {
            return Err(CandidateProjectionError::invalid(
                "privacy",
                format!(
                    "{} privacy entries for {} candidates",
                    privacy.len(),
                    self.candidates.len()
                ),
            ));
        }
        let mut private = PrivateCandidateFeatures::new();
        let candidates = self
            .candidates
            .iter()
            .zip(privacy)
            .map(|(candidate, privacy)| {
                let features = match privacy {
                    Privacy::Public => Some(candidate.features.clone()),
                    Privacy::Personal => {
                        private.insert(candidate.id.clone(), candidate.features.clone());
                        None
                    }
                };
                PublicCandidate {
                    id: candidate.id.clone(),
                    instance_index: candidate.instance_index,
                    actionable: candidate.actionable,
                    privacy: *privacy,
                    rect: candidate.rect,
                    click: candidate.click,
                    features,
                }
            })
            .collect();
        Ok((
            PublicCandidateSet {
                schema_version: self.schema_version.clone(),
                page_id: self.page_id.clone(),
                layout_id: self.layout_id.clone(),
                layout_kind: self.layout_kind,
                frame: self.frame,
                candidates,
                candidate_set_sha256: self.candidate_set_sha256.clone(),
            },
            private,
        ))
    }

    fn validate_shape(&self) -> Result<(), CandidateProjectionError> {
        validate_set_header(
            &self.schema_version,
            &self.page_id,
            &self.layout_id,
            self.frame,
            self.candidates.len(),
        )?;
        for (index, candidate) in self.candidates.iter().enumerate() {
            validate_candidate_row(
                &self.layout_id,
                self.frame,
                index,
                CandidateRow {
                    id: &candidate.id,
                    instance_index: candidate.instance_index,
                    actionable: candidate.actionable,
                    rect: candidate.rect,
                    click: candidate.click,
                    features: Some(&candidate.features),
                },
            )?;
        }
        Ok(())
    }
}

/// One candidate of the public form. A personal candidate carries no features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCandidate {
    pub id: String,
    pub instance_index: u32,
    pub actionable: bool,
    pub privacy: Privacy,
    pub rect: CandidateRect,
    pub click: CandidateRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<CandidateFeatureMap>,
}

/// The public form of one candidate set: the core set with personal features withheld.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCandidateSet {
    pub schema_version: String,
    pub page_id: String,
    pub layout_id: String,
    pub layout_kind: CandidateLayoutKind,
    pub frame: CandidateFrame,
    pub candidates: Vec<PublicCandidate>,
    pub candidate_set_sha256: String,
}

impl PublicCandidateSet {
    pub fn candidate(&self, id: &str) -> Option<&PublicCandidate> {
        self.candidates.iter().find(|candidate| candidate.id == id)
    }

    /// Checks the shape and that exactly the personal candidates withhold their features.
    /// The hash cannot be recomputed from the public form and is carried as sealed.
    pub fn validate(&self) -> Result<(), CandidateProjectionError> {
        validate_set_header(
            &self.schema_version,
            &self.page_id,
            &self.layout_id,
            self.frame,
            self.candidates.len(),
        )?;
        if !is_lower_hex_sha256(&self.candidate_set_sha256) {
            return Err(CandidateProjectionError::invalid(
                "candidate_set_sha256",
                "the candidate-set hash is not 64 lowercase hexadecimal digits",
            ));
        }
        for (index, candidate) in self.candidates.iter().enumerate() {
            if (candidate.privacy == Privacy::Personal) != candidate.features.is_none() {
                return Err(CandidateProjectionError::invalid(
                    "features",
                    format!(
                        "candidate `{}` withholds features exactly when it is not personal",
                        candidate.id
                    ),
                ));
            }
            validate_candidate_row(
                &self.layout_id,
                self.frame,
                index,
                CandidateRow {
                    id: &candidate.id,
                    instance_index: candidate.instance_index,
                    actionable: candidate.actionable,
                    rect: candidate.rect,
                    click: candidate.click,
                    features: candidate.features.as_ref(),
                },
            )?;
        }
        Ok(())
    }
}

/// The compact JSON of every public candidate set one observation carries, within
/// [`CANDIDATE_SETS_MAX_BYTES`]. Checked when the observation is built, not when it is stored.
pub fn validate_candidate_sets_budget(
    sets: &[PublicCandidateSet],
) -> Result<(), CandidateProjectionError> {
    let bytes = serde_json::to_vec(sets).map_err(|error| {
        CandidateProjectionError::invalid("candidate_sets", format!("encoding failed: {error}"))
    })?;
    if bytes.len() > CANDIDATE_SETS_MAX_BYTES {
        return Err(CandidateProjectionError {
            code: CANDIDATE_SETS_BUDGET_EXCEEDED,
            item: "candidate_sets_bytes",
            detail: format!(
                "{} bytes exceed the {CANDIDATE_SETS_MAX_BYTES}-byte observation budget",
                bytes.len()
            ),
        });
    }
    Ok(())
}

/// Formats `{layout_id}#{NN}` with a two-digit, zero-padded instance index.
pub fn candidate_id(
    layout_id: &str,
    instance_index: u32,
) -> Result<String, CandidateProjectionError> {
    validate_candidate_layout_id(layout_id)
        .map_err(|error| CandidateProjectionError::invalid_id(error.detail))?;
    if instance_index as usize >= CANDIDATE_PROJECTION_MAX_CANDIDATES {
        return Err(CandidateProjectionError::invalid_id(format!(
            "instance index {instance_index} is outside 00-63"
        )));
    }
    Ok(format!("{layout_id}#{instance_index:02}"))
}

/// Splits `{layout_id}#{NN}` into the layout ID and the instance index.
pub fn parse_candidate_id(id: &str) -> Result<(&str, u32), CandidateProjectionError> {
    let Some((layout_id, index)) = id.split_once('#') else {
        return Err(CandidateProjectionError::invalid_id(
            "a candidate ID is the layout ID, `#` and a two-digit instance index",
        ));
    };
    validate_candidate_layout_id(layout_id)
        .map_err(|error| CandidateProjectionError::invalid_id(error.detail))?;
    if index.len() != 2 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CandidateProjectionError::invalid_id(
            "the instance index is two decimal digits",
        ));
    }
    let instance_index = index
        .parse::<u32>()
        .map_err(|_| CandidateProjectionError::invalid_id("the instance index is not a number"))?;
    if instance_index as usize >= CANDIDATE_PROJECTION_MAX_CANDIDATES {
        return Err(CandidateProjectionError::invalid_id(format!(
            "instance index {instance_index} is outside 00-63"
        )));
    }
    Ok((layout_id, instance_index))
}

/// `^[a-z0-9][a-z0-9_./-]{0,63}$`: never `#` or `[`, so a candidate ID cannot collide with
/// a page element ID.
pub fn validate_candidate_layout_id(layout_id: &str) -> Result<(), CandidateProjectionError> {
    let bytes = layout_id.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= CANDIDATE_LAYOUT_ID_MAX_BYTES
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'/' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(CandidateProjectionError::invalid(
            "layout_id",
            format!("`{layout_id}` does not match ^[a-z0-9][a-z0-9_./-]{{0,63}}$"),
        ))
    }
}

/// `^[a-z][a-z0-9_]{0,31}$`.
pub fn validate_candidate_feature_name(name: &str) -> Result<(), CandidateProjectionError> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= CANDIDATE_FEATURE_NAME_MAX_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(CandidateProjectionError::invalid(
            "feature_name",
            format!("`{name}` does not match ^[a-z][a-z0-9_]{{0,31}}$"),
        ))
    }
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_set_header(
    schema_version: &str,
    page_id: &str,
    layout_id: &str,
    frame: CandidateFrame,
    candidate_count: usize,
) -> Result<(), CandidateProjectionError> {
    if schema_version != CANDIDATE_PROJECTION_SCHEMA_VERSION {
        return Err(CandidateProjectionError::invalid(
            "schema_version",
            format!("`{schema_version}` is not `{CANDIDATE_PROJECTION_SCHEMA_VERSION}`"),
        ));
    }
    if page_id.is_empty()
        || page_id.len() > PAGE_ID_MAX_BYTES
        || page_id.chars().any(char::is_control)
    {
        return Err(CandidateProjectionError::invalid(
            "page_id",
            "the page ID is empty, longer than 256 bytes or holds a control character",
        ));
    }
    validate_candidate_layout_id(layout_id)?;
    if frame.width == 0 || frame.height == 0 {
        return Err(CandidateProjectionError::invalid(
            "frame",
            "the frame has a zero dimension",
        ));
    }
    if candidate_count > CANDIDATE_PROJECTION_MAX_CANDIDATES {
        return Err(CandidateProjectionError::budget_exceeded(
            "candidates",
            format!(
                "{candidate_count} candidates exceed the {CANDIDATE_PROJECTION_MAX_CANDIDATES}-candidate budget"
            ),
        ));
    }
    Ok(())
}

struct CandidateRow<'a> {
    id: &'a str,
    instance_index: u32,
    actionable: bool,
    rect: CandidateRect,
    click: CandidateRect,
    features: Option<&'a CandidateFeatureMap>,
}

fn validate_candidate_row(
    layout_id: &str,
    frame: CandidateFrame,
    index: usize,
    row: CandidateRow<'_>,
) -> Result<(), CandidateProjectionError> {
    if row.instance_index as usize != index {
        return Err(CandidateProjectionError::invalid(
            "instance_index",
            format!(
                "candidate at position {index} carries instance index {}",
                row.instance_index
            ),
        ));
    }
    let expected = format!("{layout_id}#{index:02}");
    if row.id != expected {
        return Err(CandidateProjectionError::invalid(
            "candidate_id",
            format!("candidate `{}` is not `{expected}`", row.id),
        ));
    }
    if !row.rect.is_valid() || !row.click.is_valid() {
        return Err(CandidateProjectionError::invalid(
            "rect",
            format!("candidate `{expected}` has an empty or overflowing rectangle"),
        ));
    }
    if row.actionable && (!row.rect.is_within(frame) || !row.click.is_within(frame)) {
        return Err(CandidateProjectionError::invalid(
            "rect",
            format!("actionable candidate `{expected}` reaches outside the frame"),
        ));
    }
    let Some(features) = row.features else {
        return Ok(());
    };
    if features.len() > CANDIDATE_PROJECTION_MAX_FEATURES {
        return Err(CandidateProjectionError::budget_exceeded(
            "features",
            format!(
                "candidate `{expected}` carries {} features above the {CANDIDATE_PROJECTION_MAX_FEATURES}-feature budget",
                features.len()
            ),
        ));
    }
    if !row.actionable && !features.is_empty() {
        return Err(CandidateProjectionError::invalid(
            "features",
            format!("candidate `{expected}` is not actionable and carries features"),
        ));
    }
    for name in features.keys() {
        validate_candidate_feature_name(name)?;
    }
    Ok(())
}

#[derive(Serialize)]
struct HashedProjection<'a> {
    schema_version: &'a str,
    page_id: &'a str,
    layout_id: &'a str,
    layout_kind: CandidateLayoutKind,
    frame: CandidateFrame,
    candidates: Vec<HashedCandidate<'a>>,
}

#[derive(Serialize)]
struct HashedCandidate<'a> {
    id: &'a str,
    instance_index: u32,
    actionable: bool,
    rect: CandidateRect,
    click: CandidateRect,
    features: BTreeMap<&'a str, HashedFeature>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HashedFeature {
    Integer { value: i64 },
    Boolean { value: bool },
}

impl From<CandidateFeature> for HashedFeature {
    fn from(feature: CandidateFeature) -> Self {
        match feature {
            CandidateFeature::Integer { value, .. } => Self::Integer { value },
            CandidateFeature::Boolean { value, .. } => Self::Boolean { value },
        }
    }
}
