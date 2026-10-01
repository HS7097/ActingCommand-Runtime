// SPDX-License-Identifier: AGPL-3.0-only

//! Candidate layouts of recognition pack schema `0.7` and the one producer of
//! `actingcommand.candidate-projection.v1` (Workflow #308; `contracts/selection-graph.md`,
//! section Candidate layouts, and `contracts/candidate-projection.md`, section Generation).
//!
//! A `fixed_slots` layout declares, for one page, its slots: each slot's rectangle, the
//! rectangle an input may be sampled in, and which existing recognition target each declared
//! feature reads in that slot. [`SceneEvaluation::project_candidates`] evaluates those targets
//! on one scene and returns the contract projection with its sealed hash; the in-task select
//! step, the online observation and the offline Lab share this one entry, so the same scene and
//! pack give the same bytes everywhere. A budget is an error, never a truncation, and a value
//! the backend does not give is absent, never defaulted.

use crate::{
    PackRect, PackRegion, RecognitionEvaluator, RecognitionPack, RecognitionPackError,
    RecognitionPackErrorCode, RecognitionPackResult, RecognitionTarget, SCHEMA_0_7,
    SceneEvaluation, TargetEvaluation, TargetKind, reject_unknown_fields, validate_rect_shape,
    validate_region_within_coordinate_space,
};
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_contract::candidate_projection::{
    CANDIDATE_PROJECTION_MAX_CANDIDATES, CANDIDATE_PROJECTION_MAX_FEATURES,
    CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE, CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE,
    CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS, CandidateFeature, CandidateFeatureMap,
    CandidateFrame, CandidateLayoutKind, CandidateProjection, CandidateProjectionError,
    CandidateRect, INVALID_CANDIDATE_PROJECTION, ProjectedCandidate, candidate_id,
    validate_candidate_feature_name, validate_candidate_layout_id,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map};
use std::fmt;

/// The recognition pack declares no layout with the requested ID that this runtime projects.
pub const CANDIDATE_LAYOUT_UNKNOWN: &str = "candidate_layout_unknown";
/// A feature's target could not be evaluated; the detail names the target and the recognition
/// error code, and the recognition error travels as the cause.
pub const CANDIDATE_FEATURE_FAILED: &str = "candidate_feature_failed";
/// A feature's target needs an OCR or NN provider the evaluator was built without.
pub const CANDIDATE_FEATURE_PROVIDER_MISSING: &str = "candidate_feature_provider_missing";

const PAGE_ID_MAX_BYTES: usize = 256;
/// Larger magnitudes cannot be held by an `i64` milli value.
const MILLI_MAGNITUDE_LIMIT: f64 = 9.0e18;

/// One candidate layout of the pack's top-level `candidate_layouts` (schema `0.7`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLayout {
    pub id: String,
    /// The page the layout belongs to; the loader that holds the page set checks it with
    /// [`RecognitionPack::validate_candidate_layout_pages`].
    pub page_id: String,
    pub kind: CandidateLayoutKind,
    /// 1..=8 features, in declaration order.
    pub features: Vec<CandidateFeatureDeclaration>,
    /// 1..=64 slots; slot `k` is the candidate with instance index `k`.
    pub slots: Vec<CandidateSlot>,
}

/// One declared feature: its name and which value of its target it carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateFeatureDeclaration {
    pub name: String,
    pub value: CandidateFeatureValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateFeatureValue {
    /// The target's verdict, a boolean, for every evaluable target kind.
    Passed,
    /// The target's measure in integer milli, for every evaluable kind except composite.
    MeasureMilli,
}

/// One slot of a `fixed_slots` layout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSlot {
    pub rect: PackRect,
    pub click: PackRect,
    /// Feature name to the ID of the existing target the feature reads in this slot. A
    /// declared feature without an entry is absent from this slot's candidate.
    pub targets: BTreeMap<String, String>,
}

/// Why no candidate projection was produced. Nothing is truncated or defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateProjectionFailure {
    code: &'static str,
    item: &'static str,
    detail: String,
    cause: Option<Box<RecognitionPackError>>,
}

impl CandidateProjectionFailure {
    fn new(code: &'static str, item: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            item,
            detail: detail.into(),
            cause: None,
        }
    }

    fn feature(target_id: &str, error: RecognitionPackError) -> Self {
        let code = if error.code() == RecognitionPackErrorCode::VisionProviderMissing {
            CANDIDATE_FEATURE_PROVIDER_MISSING
        } else {
            CANDIDATE_FEATURE_FAILED
        };
        Self {
            code,
            item: "features",
            detail: format!(
                "target '{target_id}' failed with {:?}: {}",
                error.code(),
                error.message()
            ),
            cause: Some(Box::new(error)),
        }
    }

    fn unmeasurable(evaluation: &TargetEvaluation, reason: &str) -> Self {
        Self::new(
            CANDIDATE_FEATURE_FAILED,
            "features",
            format!(
                "target '{}' of kind {:?} {reason}",
                evaluation.id, evaluation.kind
            ),
        )
    }

    /// `candidate_projection_budget_exceeded`, `invalid_candidate_projection`,
    /// `invalid_candidate_id`, [`CANDIDATE_LAYOUT_UNKNOWN`], [`CANDIDATE_FEATURE_FAILED`] or
    /// [`CANDIDATE_FEATURE_PROVIDER_MISSING`].
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// The budget item or field the failure concerns, e.g. `candidates` or `features`.
    pub const fn item(&self) -> &'static str {
        self.item
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// The recognition error behind a feature failure, with its PP-OCR reports and region.
    pub fn cause(&self) -> Option<&RecognitionPackError> {
        self.cause.as_deref()
    }
}

impl From<CandidateProjectionError> for CandidateProjectionFailure {
    fn from(error: CandidateProjectionError) -> Self {
        Self::new(error.code(), error.item(), error.detail())
    }
}

impl fmt::Display for CandidateProjectionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at {}: {}", self.code, self.item, self.detail)
    }
}

impl std::error::Error for CandidateProjectionFailure {}

impl RecognitionEvaluator {
    /// The declared candidate layout with this ID.
    pub fn candidate_layout(&self, layout_id: &str) -> Option<&CandidateLayout> {
        self.pack
            .candidate_layouts
            .iter()
            .find(|layout| layout.id == layout_id)
    }
}

impl RecognitionPack {
    /// The load-site check of `page_id`: every candidate layout names a page of the package's
    /// page set. The pack carries no pages, so the loader that holds both runs this after the
    /// page set is validated against the pack.
    pub fn validate_candidate_layout_pages(
        &self,
        is_declared_page: impl Fn(&str) -> bool,
    ) -> RecognitionPackResult<()> {
        for (index, layout) in self.candidate_layouts.iter().enumerate() {
            if !is_declared_page(&layout.page_id) {
                let pointer = format!("/candidate_layouts/{index}/page_id");
                return Err(declaration(
                    pointer.clone(),
                    ResourceDeclarationReason::InvalidValue,
                    format!(
                        "{pointer}: candidate layout '{}' names page '{}', which the page set does not declare",
                        layout.id, layout.page_id
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl SceneEvaluation<'_> {
    /// The candidate set the declared layout `layout_id` yields on this scene.
    ///
    /// Slot `k` becomes candidate `{layout_id}#{k:02}`, actionable, with the slot's rectangles.
    /// Each feature the slot maps reads its target's verdict (`passed`) or measure
    /// (`measure_milli`); each distinct target is evaluated once per projection, a template
    /// through the scene's template cache. The candidate, feature and provider-evaluation
    /// budgets are checked before anything is evaluated. Any evaluation error fails the whole
    /// projection; a measure or confidence the backend does not give is absent.
    pub fn project_candidates(
        &self,
        layout_id: &str,
    ) -> Result<CandidateProjection, CandidateProjectionFailure> {
        let evaluator = self.evaluator;
        let layout = evaluator.candidate_layout(layout_id).ok_or_else(|| {
            CandidateProjectionFailure::new(
                CANDIDATE_LAYOUT_UNKNOWN,
                "layout_id",
                format!("the recognition pack declares no candidate layout '{layout_id}'"),
            )
        })?;
        if layout.kind != CandidateLayoutKind::FixedSlots {
            return Err(CandidateProjectionFailure::new(
                CANDIDATE_LAYOUT_UNKNOWN,
                "layout_kind",
                format!("candidate layout '{layout_id}' is not a fixed_slots layout"),
            ));
        }
        evaluator
            .validate_coordinate_space(self.scene)
            .map_err(|error| {
                CandidateProjectionFailure::new(
                    INVALID_CANDIDATE_PROJECTION,
                    "frame",
                    error.message(),
                )
            })?;
        if layout.slots.len() > CANDIDATE_PROJECTION_MAX_CANDIDATES {
            return Err(CandidateProjectionError::budget_exceeded(
                "candidates",
                format!(
                    "layout '{layout_id}' declares {} slots above the {CANDIDATE_PROJECTION_MAX_CANDIDATES}-candidate budget",
                    layout.slots.len()
                ),
            )
            .into());
        }
        if layout.features.len() > CANDIDATE_PROJECTION_MAX_FEATURES {
            return Err(CandidateProjectionError::budget_exceeded(
                "features",
                format!(
                    "layout '{layout_id}' declares {} features above the {CANDIDATE_PROJECTION_MAX_FEATURES}-feature budget",
                    layout.features.len()
                ),
            )
            .into());
        }
        let provider = provider_evaluations(layout, |target_id| evaluator.target(target_id).ok());
        if provider > CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS {
            return Err(CandidateProjectionError::budget_exceeded(
                "provider_evaluations",
                format!(
                    "layout '{layout_id}' needs {provider} OCR and NN evaluations above the {CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS}-evaluation budget"
                ),
            )
            .into());
        }

        let frame = CandidateFrame {
            width: self.scene.width(),
            height: self.scene.height(),
        };
        let mut evaluations = BTreeMap::<&str, TargetEvaluation>::new();
        let mut candidates = Vec::with_capacity(layout.slots.len());
        for (index, slot) in layout.slots.iter().enumerate() {
            let instance_index = u32::try_from(index).map_err(|_| {
                CandidateProjectionError::budget_exceeded(
                    "candidates",
                    format!("slot {index} has no instance index"),
                )
            })?;
            let mut features = CandidateFeatureMap::new();
            for feature in &layout.features {
                let Some(target_id) = slot.targets.get(&feature.name) else {
                    continue;
                };
                let evaluation = match evaluations.entry(target_id.as_str()) {
                    btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    btree_map::Entry::Vacant(entry) => {
                        entry.insert(self.evaluate_target(target_id).map_err(|error| {
                            CandidateProjectionFailure::feature(target_id, error)
                        })?)
                    }
                };
                if let Some(value) = feature_value(feature.value, evaluation)? {
                    features.insert(feature.name.clone(), value);
                }
            }
            candidates.push(ProjectedCandidate {
                id: candidate_id(&layout.id, instance_index)?,
                instance_index,
                actionable: true,
                rect: candidate_rect(slot.rect),
                click: candidate_rect(slot.click),
                features,
            });
        }
        CandidateProjection::new(
            layout.page_id.clone(),
            layout.id.clone(),
            layout.kind,
            frame,
            candidates,
        )
        .map_err(CandidateProjectionFailure::from)
    }
}

fn candidate_rect(rect: PackRect) -> CandidateRect {
    CandidateRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn feature_value(
    value: CandidateFeatureValue,
    evaluation: &TargetEvaluation,
) -> Result<Option<CandidateFeature>, CandidateProjectionFailure> {
    let confidence = confidence_milli(evaluation)?;
    Ok(match value {
        CandidateFeatureValue::Passed => Some(CandidateFeature::Boolean {
            value: evaluation.passed,
            confidence,
        }),
        CandidateFeatureValue::MeasureMilli => {
            measure_milli(evaluation)?.map(|value| CandidateFeature::Integer { value, confidence })
        }
    })
}

/// The backend's own confidence in integer milli: the template score, the OCR confidence and
/// the selected NN score; none for color, color digest and composite targets, and none when
/// the OCR or NN result carries none.
fn confidence_milli(
    evaluation: &TargetEvaluation,
) -> Result<Option<i64>, CandidateProjectionFailure> {
    match evaluation.kind {
        TargetKind::Template => template_score_milli(evaluation).map(Some),
        TargetKind::Ocr => ocr_confidence_milli(evaluation),
        TargetKind::Nn => nn_score_milli(evaluation),
        TargetKind::Color | TargetKind::ColorDigest | TargetKind::Composite => Ok(None),
        TargetKind::ClickOnly => Err(CandidateProjectionFailure::unmeasurable(
            evaluation,
            "cannot be evaluated",
        )),
    }
}

/// `measure_milli`: template `floor(score*1000)`, color `floor(distance*1000)`, color digest
/// `mean_milli`, OCR the shared confidence conversion and NN `floor(selected_score*1000)`.
fn measure_milli(evaluation: &TargetEvaluation) -> Result<Option<i64>, CandidateProjectionFailure> {
    match evaluation.kind {
        TargetKind::Template => template_score_milli(evaluation).map(Some),
        TargetKind::Color => {
            let color = evaluation.color.as_ref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(evaluation, "carries no color evidence")
            })?;
            floor_milli(&evaluation.id, color.distance).map(Some)
        }
        TargetKind::ColorDigest => {
            let digest = evaluation.color_digest.as_deref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(evaluation, "carries no digest evidence")
            })?;
            Ok(Some(i64::from(digest.mean_milli)))
        }
        TargetKind::Ocr => ocr_confidence_milli(evaluation),
        TargetKind::Nn => nn_score_milli(evaluation),
        TargetKind::Composite | TargetKind::ClickOnly => Err(
            CandidateProjectionFailure::unmeasurable(evaluation, "has no measure"),
        ),
    }
}

fn template_score_milli(evaluation: &TargetEvaluation) -> Result<i64, CandidateProjectionFailure> {
    let template = evaluation.template.as_ref().ok_or_else(|| {
        CandidateProjectionFailure::unmeasurable(evaluation, "carries no template evidence")
    })?;
    floor_milli(&evaluation.id, template.score)
}

/// The OCR confidence through the one shared conversion of the contract crate
/// (`contracts/resource-readings.md`), so a reading and a feature of the same OCR result agree.
fn ocr_confidence_milli(
    evaluation: &TargetEvaluation,
) -> Result<Option<i64>, CandidateProjectionFailure> {
    let ocr = evaluation.ocr.as_deref().ok_or_else(|| {
        CandidateProjectionFailure::unmeasurable(evaluation, "carries no OCR evidence")
    })?;
    Ok(actingcommand_contract::ocr_confidence_milli(ocr.confidence).map(i64::from))
}

fn nn_score_milli(
    evaluation: &TargetEvaluation,
) -> Result<Option<i64>, CandidateProjectionFailure> {
    let nn = evaluation.nn.as_ref().ok_or_else(|| {
        CandidateProjectionFailure::unmeasurable(evaluation, "carries no NN evidence")
    })?;
    nn.selected_score
        .map(|score| floor_milli(&evaluation.id, score))
        .transpose()
}

/// `floor(value * 1000)`, computed in `f64`, for template, color and NN values.
fn floor_milli(target_id: &str, value: f32) -> Result<i64, CandidateProjectionFailure> {
    let milli = (f64::from(value) * 1000.0).floor();
    if !milli.is_finite() || milli.abs() >= MILLI_MAGNITUDE_LIMIT {
        return Err(CandidateProjectionFailure::new(
            CANDIDATE_FEATURE_FAILED,
            "features",
            format!("target '{target_id}' gave {value}, which has no integer milli value"),
        ));
    }
    Ok(milli as i64)
}

/// OCR and NN evaluations one projection of `layout` performs: one per distinct OCR or NN
/// target its slots read, and one per OCR or NN member of each distinct composite they read.
fn provider_evaluations<'t>(
    layout: &CandidateLayout,
    target: impl Fn(&str) -> Option<&'t RecognitionTarget>,
) -> usize {
    let mut distinct = BTreeSet::new();
    for slot in &layout.slots {
        for feature in &layout.features {
            if let Some(target_id) = slot.targets.get(&feature.name) {
                distinct.insert(target_id.as_str());
            }
        }
    }
    let is_provider = |target_id: &str| {
        matches!(
            target(target_id),
            Some(RecognitionTarget::Ocr(_) | RecognitionTarget::Nn(_))
        )
    };
    distinct
        .into_iter()
        .map(|target_id| match target(target_id) {
            Some(RecognitionTarget::Ocr(_) | RecognitionTarget::Nn(_)) => 1,
            Some(RecognitionTarget::Composite(composite)) => composite
                .members
                .iter()
                .filter(|member| is_provider(member.as_str()))
                .count(),
            _ => 0,
        })
        .sum()
}

/// The wire shape of the schema `0.7` top-level `candidate_layouts`: unknown fields, wrong
/// JSON types, and a layout kind or feature value this runtime does not know are refused with
/// the pointer of the offending field.
pub(crate) fn validate_candidate_layouts_wire(value: &Value) -> RecognitionPackResult<()> {
    let layouts = wire_array(value, "/candidate_layouts")?;
    for (index, layout) in layouts.iter().enumerate() {
        let pointer = format!("/candidate_layouts/{index}");
        let layout = wire_object(
            layout,
            &["id", "page_id", "kind", "features", "slots"],
            &pointer,
        )?;
        if let Some(kind) = layout.get("kind")
            && kind.as_str() != Some("fixed_slots")
        {
            return Err(declaration(
                format!("{pointer}/kind"),
                ResourceDeclarationReason::InvalidValue,
                format!(
                    "{pointer}/kind {kind} is not a layout kind this runtime projects; expected \"fixed_slots\""
                ),
            ));
        }
        if let Some(features) = layout.get("features") {
            let features_pointer = format!("{pointer}/features");
            for (feature_index, feature) in
                wire_array(features, &features_pointer)?.iter().enumerate()
            {
                let feature_pointer = format!("{features_pointer}/{feature_index}");
                let feature = wire_object(feature, &["name", "value"], &feature_pointer)?;
                if let Some(value) = feature.get("value")
                    && !matches!(value.as_str(), Some("passed" | "measure_milli"))
                {
                    return Err(declaration(
                        format!("{feature_pointer}/value"),
                        ResourceDeclarationReason::InvalidValue,
                        format!(
                            "{feature_pointer}/value {value} is not \"passed\" or \"measure_milli\""
                        ),
                    ));
                }
            }
        }
        if let Some(slots) = layout.get("slots") {
            let slots_pointer = format!("{pointer}/slots");
            for (slot_index, slot) in wire_array(slots, &slots_pointer)?.iter().enumerate() {
                let slot_pointer = format!("{slots_pointer}/{slot_index}");
                let slot = wire_object(slot, &["rect", "click", "targets"], &slot_pointer)?;
                for field in ["rect", "click"] {
                    if let Some(rect) = slot.get(field) {
                        wire_object(
                            rect,
                            &["x", "y", "width", "height"],
                            &format!("{slot_pointer}/{field}"),
                        )?;
                    }
                }
                if let Some(targets) = slot.get("targets") {
                    let targets_pointer = format!("{slot_pointer}/targets");
                    let targets = targets.as_object().ok_or_else(|| {
                        declaration(
                            targets_pointer.clone(),
                            ResourceDeclarationReason::InvalidType,
                            format!("{targets_pointer} must be an object"),
                        )
                    })?;
                    for (name, target_id) in targets {
                        if !target_id.is_string() {
                            let target_pointer =
                                format!("{targets_pointer}/{}", pointer_token(name));
                            return Err(declaration(
                                target_pointer.clone(),
                                ResourceDeclarationReason::InvalidType,
                                format!("{target_pointer} must be a target ID string"),
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// The package-internal rules of the declared candidate layouts, as messages that start with
/// the pointer of the offending field: IDs, counts and budgets, rectangles inside the
/// coordinate space, and feature targets that exist and can be evaluated. A layout's page is
/// checked where the page set is loaded ([`RecognitionPack::validate_candidate_layout_pages`]).
pub(crate) fn validate_candidate_layouts(pack: &RecognitionPack, errors: &mut Vec<String>) {
    let layouts = &pack.candidate_layouts;
    if layouts.is_empty() {
        return;
    }
    if pack.schema_version != SCHEMA_0_7 {
        errors.push(format!(
            "/candidate_layouts requires schema_version '{SCHEMA_0_7}', got '{}'",
            pack.schema_version
        ));
    }
    if layouts.len() > CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE {
        errors.push(format!(
            "/candidate_layouts declares {} layouts above the {CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE}-layout package budget",
            layouts.len()
        ));
    }
    let mut targets = HashMap::new();
    for target in &pack.targets {
        targets.entry(target.id()).or_insert(target);
    }
    let mut layout_ids = HashSet::new();
    let mut page_layouts = HashMap::<&str, usize>::new();
    for (index, layout) in layouts.iter().enumerate() {
        let pointer = format!("/candidate_layouts/{index}");
        if let Err(error) = validate_candidate_layout_id(&layout.id) {
            errors.push(format!("{pointer}/id: {}", error.detail()));
        } else if !layout_ids.insert(layout.id.as_str()) {
            errors.push(format!(
                "{pointer}/id: candidate layout '{}' is declared more than once",
                layout.id
            ));
        }
        if layout.page_id.is_empty()
            || layout.page_id.len() > PAGE_ID_MAX_BYTES
            || layout.page_id.chars().any(char::is_control)
        {
            errors.push(format!(
                "{pointer}/page_id: the page ID is empty, longer than {PAGE_ID_MAX_BYTES} bytes or holds a control character"
            ));
        }
        let page_count = page_layouts.entry(layout.page_id.as_str()).or_default();
        *page_count += 1;
        if *page_count == CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE + 1 {
            errors.push(format!(
                "{pointer}/page_id: page '{}' declares more than {CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE} candidate layouts",
                layout.page_id
            ));
        }
        if layout.kind != CandidateLayoutKind::FixedSlots {
            errors.push(format!(
                "{pointer}/kind: this runtime projects fixed_slots layouts only"
            ));
        }
        if !(1..=CANDIDATE_PROJECTION_MAX_FEATURES).contains(&layout.features.len()) {
            errors.push(format!(
                "{pointer}/features must declare 1..={CANDIDATE_PROJECTION_MAX_FEATURES} features, got {}",
                layout.features.len()
            ));
        }
        let mut features = HashMap::new();
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let Err(error) = validate_candidate_feature_name(&feature.name) {
                errors.push(format!(
                    "{pointer}/features/{feature_index}/name: {}",
                    error.detail()
                ));
            } else if features
                .insert(feature.name.as_str(), feature.value)
                .is_some()
            {
                errors.push(format!(
                    "{pointer}/features/{feature_index}/name: feature '{}' is declared more than once",
                    feature.name
                ));
            }
        }
        if !(1..=CANDIDATE_PROJECTION_MAX_CANDIDATES).contains(&layout.slots.len()) {
            errors.push(format!(
                "{pointer}/slots must declare 1..={CANDIDATE_PROJECTION_MAX_CANDIDATES} slots, got {}",
                layout.slots.len()
            ));
        }
        for (slot_index, slot) in layout.slots.iter().enumerate() {
            let slot_pointer = format!("{pointer}/slots/{slot_index}");
            for (field, rect) in [("rect", slot.rect), ("click", slot.click)] {
                let label = format!("{slot_pointer}/{field}");
                validate_rect_shape(rect, &label, errors);
                validate_region_within_coordinate_space(
                    &PackRegion::Rect(rect),
                    pack.coordinate_space,
                    &label,
                    errors,
                );
            }
            for (name, target_id) in &slot.targets {
                let target_pointer = format!("{slot_pointer}/targets/{}", pointer_token(name));
                let Some(value) = features.get(name.as_str()) else {
                    errors.push(format!(
                        "{target_pointer}: '{name}' is not a feature of candidate layout '{}'",
                        layout.id
                    ));
                    continue;
                };
                match targets.get(target_id.as_str()).copied() {
                    None => errors.push(format!(
                        "{target_pointer}: target '{target_id}' does not exist"
                    )),
                    Some(RecognitionTarget::ClickOnly(_)) => errors.push(format!(
                        "{target_pointer}: target '{target_id}' is click-only and cannot be evaluated"
                    )),
                    Some(RecognitionTarget::Composite(_))
                        if *value == CandidateFeatureValue::MeasureMilli =>
                    {
                        errors.push(format!(
                            "{target_pointer}: composite target '{target_id}' has no measure; a composite feature is 'passed'"
                        ))
                    }
                    Some(_) => {}
                }
            }
        }
        let provider = provider_evaluations(layout, |target_id| targets.get(target_id).copied());
        if provider > CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS {
            errors.push(format!(
                "{pointer}/slots: {provider} OCR and NN evaluations exceed the {CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS}-evaluation projection budget"
            ));
        }
    }
}

fn wire_array<'v>(value: &'v Value, pointer: &str) -> RecognitionPackResult<&'v Vec<Value>> {
    value.as_array().ok_or_else(|| {
        declaration(
            pointer.to_owned(),
            ResourceDeclarationReason::InvalidType,
            format!("{pointer} must be an array"),
        )
    })
}

fn wire_object<'v>(
    value: &'v Value,
    allowed: &[&str],
    pointer: &str,
) -> RecognitionPackResult<&'v Map<String, Value>> {
    let object = value.as_object().ok_or_else(|| {
        declaration(
            pointer.to_owned(),
            ResourceDeclarationReason::InvalidType,
            format!("{pointer} must be an object"),
        )
    })?;
    reject_unknown_fields(object, allowed, pointer)?;
    Ok(object)
}

fn declaration(
    pointer: String,
    reason: ResourceDeclarationReason,
    message: String,
) -> RecognitionPackError {
    RecognitionPackError::fatal(message).at_declaration(pointer, reason)
}

fn pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}
