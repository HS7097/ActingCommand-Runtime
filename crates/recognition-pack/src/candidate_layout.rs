// SPDX-License-Identifier: AGPL-3.0-only

//! Candidate layouts of recognition pack schema `0.7` and the one producer of
//! `actingcommand.candidate-projection.v1` (Workflow #308; `contracts/selection-graph.md`,
//! section Candidate layouts, and `contracts/candidate-projection.md`, section Generation).
//!
//! A `fixed_slots` layout declares, for one page, its slots: each slot's rectangle, the
//! rectangle an input may be sampled in, and which existing recognition target each declared
//! feature reads in that slot. A `repeated_anchor` layout declares one anchor template instead:
//! every accepted match of the anchor on the frame is an instance, whose rectangle, click and
//! feature regions lie at declared offsets from the match. An instance is actionable, and its
//! features are evaluated, only when its rectangle and its click lie inside the layout's
//! readable band, the whole frame by default. A feature of either kind may read one target at
//! an `offset` from its instance's origin. [`SceneEvaluation::project_candidates`] evaluates
//! those targets on one scene and returns the contract projection with its sealed hash; the
//! in-task select step, the online observation and the offline Lab share this one entry, so the
//! same scene and pack give the same bytes everywhere. A budget is an error, never a
//! truncation, and a value the backend does not give is absent, never defaulted.

use crate::codes::{RecognitionPackCode, RecognitionPackLocation};
use crate::{
    CandidateConsensus, CandidateIdentityDeclaration, CandidateIdentityRecognition,
    CandidateIdentityTemplate, CandidateSampleVariant, PackPoint, PackRect, PackRegion,
    RecognitionEvaluator, RecognitionPack, RecognitionPackError, RecognitionPackErrorCode,
    RecognitionPackResult, RecognitionTarget, SCHEMA_0_7, SceneEvaluation, TargetEvaluation,
    TargetKind, TemplateEvaluation, TemplateRelativeRegion, primitive_error, rect_is_within,
    reject_unknown_fields, resolve_template_relative_region, target_region, template_message,
    unsupported_template_reason, validate_rect_shape, validate_region_within_coordinate_space,
};
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_contract::candidate_projection::{
    CANDIDATE_PROJECTION_MAX_CANDIDATES, CANDIDATE_PROJECTION_MAX_FEATURES,
    CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PACKAGE, CANDIDATE_PROJECTION_MAX_LAYOUTS_PER_PAGE,
    CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS, CandidateFeature, CandidateFeatureMap,
    CandidateFrame, CandidateLayoutKind, CandidateProjection, CandidateProjectionError,
    CandidateRecognitionEvidence, CandidateRecognitionInput, CandidateRecognitionSample,
    CandidateRect, CandidateUnknownReason, INVALID_CANDIDATE_PROJECTION, ProjectedCandidate,
    candidate_id, validate_candidate_feature_name, validate_candidate_layout_id,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

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
/// The highest overlap suppression a `repeated_anchor` layout may declare.
const MAX_SUPPRESS_IOU_MILLI: u16 = 999;

const LAYOUT_FIELDS: &[&str] = &[
    "id",
    "page_id",
    "kind",
    "anchor",
    "max_instances",
    "order",
    "suppress_iou_milli",
    "instance_rect",
    "click",
    "readable_band",
    "features",
    "slots",
    "unknown_identity",
    "sample_interval_ms",
];
const FIXED_SLOTS_FIELDS: &[&str] = &[
    "id",
    "page_id",
    "kind",
    "features",
    "slots",
    "unknown_identity",
    "sample_interval_ms",
];
const REPEATED_ANCHOR_FIELDS: &[&str] = &[
    "id",
    "page_id",
    "kind",
    "anchor",
    "max_instances",
    "order",
    "suppress_iou_milli",
    "instance_rect",
    "click",
    "readable_band",
    "features",
    "unknown_identity",
    "sample_interval_ms",
];
const FEATURE_FIELDS: &[&str] = &[
    "name",
    "value",
    "identity",
    "consensus",
    "integer",
    "target",
    "offset",
];

/// One candidate layout of the pack's top-level `candidate_layouts` (schema `0.7`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLayout {
    pub id: String,
    /// The page the layout belongs to; the loader that holds the page set checks it with
    /// [`RecognitionPack::validate_candidate_layout_pages`].
    pub page_id: String,
    pub kind: CandidateLayoutKind,
    /// `repeated_anchor`: the template target whose accepted matches are the instances; its
    /// region is the search region, and its threshold and relative color check apply.
    #[serde(default)]
    pub anchor: Option<String>,
    /// `repeated_anchor`: 1..=64; more accepted matches fail the projection.
    #[serde(default)]
    pub max_instances: Option<u32>,
    /// `repeated_anchor`: how instance indexes are assigned.
    #[serde(default)]
    pub order: Option<CandidateOrder>,
    /// `repeated_anchor`: overlap suppression in milli, 0..=999; absent means 0, which
    /// suppresses any overlap.
    #[serde(default)]
    pub suppress_iou_milli: Option<u16>,
    /// `repeated_anchor`: the instance rectangle, relative to the anchor match's top-left
    /// corner.
    #[serde(default)]
    pub instance_rect: Option<PackRect>,
    /// `repeated_anchor`: where an input may be sampled, relative to the anchor match's
    /// top-left corner.
    #[serde(default)]
    pub click: Option<PackRect>,
    /// `repeated_anchor`: the frame rectangle an instance must lie in to be actionable; absent
    /// means the whole frame.
    #[serde(default)]
    pub readable_band: Option<PackRect>,
    /// 1..=8 features, in declaration order.
    pub features: Vec<CandidateFeatureDeclaration>,
    /// `fixed_slots`: 1..=64 slots; slot `k` is the candidate with instance index `k`.
    #[serde(default)]
    pub slots: Vec<CandidateSlot>,
    #[serde(default)]
    pub unknown_identity: UnknownIdentityHandling,
    /// Required for multiple capture frames; one bounded interval between successive frames.
    #[serde(default)]
    pub sample_interval_ms: u16,
}

/// How a `repeated_anchor` layout numbers its instances: `top_to_bottom` by (`y`, `x`),
/// `left_to_right` by (`x`, `y`) of the anchor match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateOrder {
    TopToBottom,
    LeftToRight,
}

/// The result of [`SceneEvaluation::evaluate_target_all`].
#[derive(Debug, Clone, PartialEq)]
pub enum TargetSearch {
    /// Every accepted match after overlap suppression, in acceptance order: score descending,
    /// then `y`, then `x`.
    Complete(Vec<TemplateEvaluation>),
    /// The search reached its time limit; nothing partial is returned. `found` counts the
    /// positions that had passed the threshold and the relative color check by then.
    Incomplete { found: usize, timeout_ms: u64 },
}

/// Only explicit resource opt-in allows ranking an unknown identity on fresh attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownIdentityHandling {
    #[default]
    Reject,
    ReadableAttributes,
}

/// One declared feature: its name and which value of its target it carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateFeatureDeclaration {
    pub name: String,
    pub value: CandidateFeatureValue,
    #[serde(default)]
    pub identity: Option<CandidateIdentityDeclaration>,
    #[serde(default)]
    pub consensus: Option<CandidateConsensus>,
    #[serde(default)]
    pub integer: Option<CandidateIntegerDeclaration>,
    /// The target this feature reads at `offset` from each instance's origin: the anchor
    /// match's top-left corner, or a slot rectangle's origin. The region keeps the target's own
    /// size. Declared with `offset`, and then never mapped by a slot.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub offset: Option<PackPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIntegerDeclaration {
    pub min: u64,
    pub max: u64,
    #[serde(default)]
    pub format: actingcommand_contract::OcrUnsignedIntegerFormat,
    pub minimum_confidence_milli: u16,
}

impl CandidateIntegerDeclaration {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.min > self.max
            || self.max > actingcommand_contract::MAX_RESOURCE_READING_VALUE
            || !(1..=1000).contains(&self.minimum_confidence_milli)
        {
            Err("invalid OCR integer bounds or confidence")
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateFeatureValue {
    /// The target's verdict, a boolean, for every evaluable target kind.
    Passed,
    /// The target's measure in integer milli, for every evaluable kind except composite.
    MeasureMilli,
    Identity,
    OcrInteger,
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
    #[serde(default)]
    pub identity_templates: BTreeMap<String, Vec<CandidateIdentityTemplate>>,
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
    pub fn recording_failed(detail: impl Into<String>) -> Self {
        Self::new(
            "candidate_sample_record_failed",
            "recognition_evidence",
            detail,
        )
    }

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

    /// `candidate_search_incomplete`: the anchor search ran out of time.
    fn search_incomplete(found: usize, timeout_ms: u64) -> Self {
        Self::new(
            RecognitionPackCode::CandidateSearchIncomplete.as_str(),
            "anchor",
            format!(
                "stage={} timeout_ms={timeout_ms} count={found}",
                RecognitionPackLocation::CandidateSearchRepeatedAnchor.as_str()
            ),
        )
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
    /// `invalid_candidate_id`, [`CANDIDATE_LAYOUT_UNKNOWN`], [`CANDIDATE_FEATURE_FAILED`],
    /// [`CANDIDATE_FEATURE_PROVIDER_MISSING`] or `candidate_search_incomplete`.
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
    /// A `repeated_anchor` layout enumerates the anchor's matches instead: the `k`-th match in
    /// the declared order becomes candidate `{layout_id}#{k:02}`, actionable only when its
    /// rectangle and click lie inside the readable band, and only an actionable instance has
    /// its features evaluated. Each feature reads its target's verdict (`passed`) or measure
    /// (`measure_milli`); each distinct target is evaluated once per projection, a template
    /// through the scene's template cache, and a target read at an offset once per instance.
    /// The candidate, feature and provider-evaluation budgets are checked before any feature is
    /// evaluated. Any evaluation error fails the whole projection; a measure or confidence the
    /// backend does not give is absent.
    pub fn project_candidates(
        &self,
        layout_id: &str,
    ) -> Result<CandidateProjection, CandidateProjectionFailure> {
        self.project_candidates_with_samples(layout_id, &[self.scene], &mut |_, _, _, _| Ok(()))
    }

    /// The supplied scenes are one observation transaction, oldest first. A caller incapable
    /// of supplying the declared frame set gets an explicit refusal before any recognition.
    /// The callback records each actual backend result (including an error) before it is used.
    pub fn project_candidates_with_samples(
        &self,
        layout_id: &str,
        scenes: &[&crate::Scene],
        record: &mut impl FnMut(
            &str,
            &RecognitionPackResult<TargetEvaluation>,
            Instant,
            Instant,
        ) -> Result<(), CandidateProjectionFailure>,
    ) -> Result<CandidateProjection, CandidateProjectionFailure> {
        let evaluator = self.evaluator;
        let layout = evaluator.candidate_layout(layout_id).ok_or_else(|| {
            CandidateProjectionFailure::new(
                CANDIDATE_LAYOUT_UNKNOWN,
                "layout_id",
                format!("the recognition pack declares no candidate layout '{layout_id}'"),
            )
        })?;
        if scenes.len() != layout.required_frames() || scenes.is_empty() || scenes.len() > 5 {
            return Err(CandidateProjectionFailure::new(
                "candidate_samples_unavailable",
                "frames",
                format!(
                    "layout '{layout_id}' requires {} frames, got {}",
                    layout.required_frames(),
                    scenes.len()
                ),
            ));
        }
        for scene in scenes {
            if scene.width() != self.scene.width() || scene.height() != self.scene.height() {
                return Err(CandidateProjectionFailure::new(
                    "candidate_sample_geometry_changed",
                    "frame",
                    "sample geometry differs within this transaction",
                ));
            }
            evaluator
                .validate_coordinate_space(scene)
                .map_err(|error| CandidateProjectionFailure::feature(layout_id, error))?;
        }
        if scenes.iter().enumerate().any(|(index, scene)| {
            scenes[..index]
                .iter()
                .any(|prior| std::ptr::eq(*prior, *scene))
        }) {
            return Err(CandidateProjectionFailure::new(
                "candidate_samples_unavailable",
                "frames",
                "frame references must name distinct captures",
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
        let provider =
            provider_evaluations(layout, evaluator.pack.defaults.match_metric, |target_id| {
                evaluator.target(target_id).ok()
            });
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
        let contexts = scenes
            .iter()
            .map(|scene| {
                let mut context = evaluator.scene_context(scene);
                context.sample_deadline = self.sample_deadline;
                context
            })
            .collect::<Vec<_>>();
        let frame_hashes = (layout.has_consensus()
            || layout
                .features
                .iter()
                .any(|feature| feature.identity.is_some() || feature.integer.is_some()))
        .then(|| {
            scenes
                .iter()
                .map(|scene| format!("{:x}", Sha256::digest(scene.rgb8_pixels())))
                .collect::<Vec<_>>()
        });
        let instances = match layout.kind {
            CandidateLayoutKind::FixedSlots => layout
                .slots
                .iter()
                .map(|slot| LayoutInstance {
                    rect: candidate_rect(slot.rect),
                    click: candidate_rect(slot.click),
                    actionable: true,
                    origin: PackPoint {
                        x: slot.rect.x,
                        y: slot.rect.y,
                    },
                    slot: Some(slot),
                })
                .collect::<Vec<_>>(),
            CandidateLayoutKind::RepeatedAnchor => {
                let instances = self.anchor_instances(layout)?;
                // `provider` counts one instance; each actionable instance repeats it.
                let needed = provider.saturating_mul(
                    instances
                        .iter()
                        .filter(|instance| instance.actionable)
                        .count(),
                );
                if needed > CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS {
                    return Err(CandidateProjectionError::budget_exceeded(
                        "provider_evaluations",
                        format!(
                            "layout_id={layout_id} count={needed} limit_count={CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS}"
                        ),
                    )
                    .into());
                }
                instances
            }
        };
        let mut evaluations = BTreeMap::<SampleKey, TargetEvaluation>::new();
        let mut evidence = Vec::new();
        let mut candidates = Vec::with_capacity(instances.len());
        for (index, instance) in instances.iter().enumerate() {
            let instance_index = u32::try_from(index).map_err(|_| {
                CandidateProjectionError::budget_exceeded(
                    "candidates",
                    format!("slot {index} has no instance index"),
                )
            })?;
            let mut features = CandidateFeatureMap::new();
            // An instance that is not actionable carries no features.
            for feature in layout.features.iter().filter(|_| instance.actionable) {
                let samples = layout.feature_samples(feature);
                let placement = layout.feature_placement(feature, instance.origin)?;
                let mut observed = Vec::with_capacity(samples.len());
                for sample in &samples {
                    let mut evaluate = |target_id: &str,
                                        at: Option<PackPoint>|
                     -> Result<TargetEvaluation, CandidateProjectionFailure> {
                        let key = sample_key(
                            sample.effective(evaluator.pack.defaults.match_metric),
                            target_id,
                            at,
                        );
                        if let Some(value) = evaluations.get(&key) {
                            return Ok(value.clone());
                        }
                        let started = Instant::now();
                        let frame_index = usize::from(sample.frame);
                        let in_frame = std::ptr::eq(scenes[frame_index], self.scene);
                        let mut result = match at {
                            Some(origin) if in_frame => {
                                self.evaluate_placed(target_id, origin, *sample)
                            }
                            Some(origin) => {
                                contexts[frame_index].evaluate_placed(target_id, origin, *sample)
                            }
                            None if in_frame => {
                                if feature.consensus.is_some() {
                                    self.evaluate_candidate_sample(target_id, *sample)
                                } else {
                                    self.evaluate_target(target_id)
                                }
                            }
                            None => {
                                let context = &contexts[frame_index];
                                if feature.consensus.is_some() {
                                    context.evaluate_candidate_sample(target_id, *sample)
                                } else {
                                    context.evaluate_target(target_id)
                                }
                            }
                        };
                        let ended = Instant::now();
                        if let Some(hashes) = &frame_hashes {
                            let sample_data =
                                actingcommand_contract::TaskDiagnosticSampleData {
                                    frame_index: sample.frame,
                                    frame_rgb8_sha256: hashes[usize::from(sample.frame)]
                                        .clone(),
                                    dx: sample.dx,
                                    dy: sample.dy,
                                    template_metric: matches!(
                                        evaluator.target(target_id),
                                        Ok(RecognitionTarget::Template(_))
                                    )
                                    .then(|| {
                                        match sample
                                            .template_metric
                                            .unwrap_or(evaluator.pack.defaults.match_metric)
                                        {
                                            crate::RecognitionMatchMetric::CcorrNormed => {
                                                "ccorr_normed"
                                            }
                                            crate::RecognitionMatchMetric::CcoeffNormed => {
                                                "ccoeff_normed"
                                            }
                                        }
                                        .to_owned()
                                    }),
                                    elapsed_us: u64::try_from(
                                        ended.duration_since(started).as_micros(),
                                    )
                                    .unwrap_or(u64::MAX),
                                    passed: result.as_ref().ok().map(|value| value.passed),
                                };
                            match &mut result {
                                Ok(value) => value.sampling = Some(actingcommand_contract::TaskDiagnosticSamplingData::Sample { sample: sample_data }),
                                Err(error) => error.sample = Some(Box::new(sample_data)),
                            }
                        }
                        record(target_id, &result, started, ended)?;
                        let result = result.map_err(|error| {
                            CandidateProjectionFailure::feature(target_id, error)
                        })?;
                        evaluations.insert(key, result.clone());
                        Ok(result)
                    };
                    let (value, input) = if let Some(identity) = &feature.identity
                        && matches!(
                            identity.recognition,
                            CandidateIdentityRecognition::IconTemplates { .. }
                        ) {
                        let templates = instance
                            .slot
                            .and_then(|slot| slot.identity_templates.get(&feature.name))
                            .ok_or_else(|| {
                                CandidateProjectionFailure::new(
                                    CANDIDATE_FEATURE_FAILED,
                                    "identity_templates",
                                    "admitted identity pool is missing",
                                )
                            })?;
                        let mut scores = Vec::with_capacity(templates.len());
                        for template in templates {
                            let evaluation = evaluate(&template.target_id, None)?;
                            let score = template_score_milli(&evaluation)?;
                            scores.push(
                                u16::try_from(score)
                                    .ok()
                                    .filter(|score| *score <= 1000)
                                    .ok_or_else(|| {
                                        CandidateProjectionFailure::unmeasurable(
                                            &evaluation,
                                            "has an invalid normalized template score",
                                        )
                                    })?,
                            );
                        }
                        (
                            Some(identity.map_icons(templates, &scores)),
                            CandidateRecognitionInput::Icons {
                                scores_milli: scores,
                            },
                        )
                    } else if let Some(target_id) = feature.target.as_ref().or_else(|| {
                        instance
                            .slot
                            .and_then(|slot| slot.targets.get(&feature.name))
                    }) {
                        let evaluation = evaluate(target_id, placement)?;
                        let value = feature_value(feature, &evaluation)?;
                        let input = if feature.identity.is_some() || feature.integer.is_some() {
                            let ocr = evaluation.ocr.as_deref().ok_or_else(|| {
                                CandidateProjectionFailure::unmeasurable(
                                    &evaluation,
                                    "has no identity OCR evidence",
                                )
                            })?;
                            CandidateRecognitionInput::Ocr {
                                text: ocr.text.clone(),
                                confidence_milli: actingcommand_contract::ocr_confidence_milli(
                                    ocr.confidence,
                                ),
                            }
                        } else {
                            CandidateRecognitionInput::Scalar {
                                value: value.clone(),
                            }
                        };
                        (value, input)
                    } else {
                        (None, CandidateRecognitionInput::Scalar { value: None })
                    };
                    if value.is_some()
                        || feature.consensus.is_some()
                        || feature.identity.is_some()
                        || feature.integer.is_some()
                    {
                        observed.push(CandidateRecognitionSample {
                            frame_index: sample.frame,
                            frame_rgb8_sha256: frame_hashes
                                .as_ref()
                                .map(|hashes| hashes[usize::from(sample.frame)].clone())
                                .unwrap_or_default(),
                            input,
                            value: value.unwrap_or(CandidateFeature::Unknown {
                                reason: CandidateUnknownReason::Missing,
                                source: feature
                                    .identity
                                    .as_ref()
                                    .map(CandidateIdentityDeclaration::source),
                            }),
                        });
                    }
                }
                let value = match &feature.consensus {
                    Some(consensus) => Some(
                        consensus.aggregate(
                            &observed
                                .iter()
                                .map(|sample| sample.value.clone())
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    None => observed.first().map(|sample| sample.value.clone()),
                };
                if let Some(mut value) = value {
                    if let CandidateFeature::Unknown { source, .. } = &mut value {
                        *source = feature
                            .identity
                            .as_ref()
                            .map(CandidateIdentityDeclaration::source);
                    }
                    features.insert(feature.name.clone(), value);
                    if feature.consensus.is_some()
                        || feature.identity.is_some()
                        || feature.integer.is_some()
                    {
                        evidence.push(CandidateRecognitionEvidence {
                            candidate_id: candidate_id(&layout.id, instance_index)?,
                            feature: feature.name.clone(),
                            samples: observed,
                        });
                    }
                }
            }
            candidates.push(ProjectedCandidate {
                id: candidate_id(&layout.id, instance_index)?,
                instance_index,
                actionable: instance.actionable,
                rect: instance.rect,
                click: instance.click,
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
        .and_then(|projection| projection.with_recognition_evidence(evidence))
        .map_err(CandidateProjectionFailure::from)
    }
}

impl SceneEvaluation<'_> {
    /// Every accepted match of the template target `target_id` on this scene, for a
    /// `repeated_anchor` layout's anchor (Workflow #308).
    ///
    /// The target's region is the search region, and its threshold and a `template_relative`
    /// color check apply to every position; a color check at a fixed rectangle cannot follow
    /// the matches and is refused. Positions are scored exactly, never through the downsampled
    /// pyramid, and overlapping positions are suppressed by `suppress_iou_milli`
    /// ([`actingcommand_recognition::Scene::match_template_all`]). A search that runs out of its
    /// time limit is [`TargetSearch::Incomplete`], never a partial list.
    pub fn evaluate_target_all(
        &self,
        target_id: &str,
        suppress_iou_milli: u16,
    ) -> RecognitionPackResult<TargetSearch> {
        if self
            .sample_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(RecognitionPackError::fatal(
                "recognition sample deadline exceeded before target evaluation",
            ));
        }
        let evaluator = self.evaluator;
        evaluator.validate_coordinate_space(self.scene)?;
        let RecognitionTarget::Template(target) = evaluator.target(target_id)? else {
            return Err(RecognitionPackError::fatal(format!(
                "instance anchor '{target_id}' is not a template target"
            )));
        };
        if let Some(reason) = unsupported_template_reason(target) {
            return Err(RecognitionPackError::fatal(format!(
                "template target '{}' uses unsupported recognition semantics: {reason}",
                target.id
            )));
        }
        let relative = match &target.color_check {
            None => None,
            Some(check) => match &check.region {
                PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                    offset,
                    width,
                    height,
                    ..
                }) => Some((check, *offset, *width, *height)),
                _ => {
                    return Err(RecognitionPackError::fatal(format!(
                        "instance anchor '{}' declares a color_check that is not template_relative",
                        target.id
                    )));
                }
            },
        };
        let template_png = evaluator
            .asset_resolver
            .read_asset(&target.template_path)
            .map_err(|err| {
                RecognitionPackError::fatal(format!(
                    "failed to read template '{}' for target '{}': {}",
                    target.template_path,
                    target.id,
                    err.message()
                ))
            })?;
        let region = target_region(&target.id, &target.region)?;
        let threshold = target
            .threshold
            .unwrap_or(evaluator.pack.defaults.template_threshold);
        let scene = self.scene;
        let search = scene
            .match_template_all_with_clock(
                &template_png,
                region,
                evaluator.default_match_metric(),
                threshold,
                suppress_iou_milli,
                |candidate| {
                    let Some((check, offset, width, height)) = relative else {
                        return Ok(true);
                    };
                    match resolve_template_relative_region(
                        scene,
                        PackPoint {
                            x: candidate.x,
                            y: candidate.y,
                        },
                        offset,
                        width,
                        height,
                    ) {
                        Ok(region) => {
                            Ok(scene.compare_color(region.into(), check.expected)?.distance
                                <= evaluator.color_max_distance(check.max_distance))
                        }
                        Err(_) => Ok(false),
                    }
                },
                self.search_clock,
            )
            .map_err(|err| primitive_error(&target.id, err))?;
        Ok(match search {
            actingcommand_recognition::TemplateSearch::Complete(matches) => TargetSearch::Complete(
                matches
                    .into_iter()
                    .map(|matched| TemplateEvaluation {
                        x: matched.x,
                        y: matched.y,
                        width: matched.width,
                        height: matched.height,
                        raw_score: matched.raw_score,
                        score: matched.score,
                        threshold,
                    })
                    .collect(),
            ),
            actingcommand_recognition::TemplateSearch::Incomplete { found, limit } => {
                TargetSearch::Incomplete {
                    found,
                    timeout_ms: u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
                }
            }
        })
    }

    /// The instances of a `repeated_anchor` layout on this scene, in the declared order: each
    /// accepted anchor match places the instance rectangle and click at their offsets, and an
    /// instance is actionable when both lie inside the readable band.
    fn anchor_instances<'l>(
        &self,
        layout: &'l CandidateLayout,
    ) -> Result<Vec<LayoutInstance<'l>>, CandidateProjectionFailure> {
        let (Some(anchor), Some(max_instances), Some(order), Some(instance_rect), Some(click)) = (
            layout.anchor.as_deref(),
            layout.max_instances,
            layout.order,
            layout.instance_rect,
            layout.click,
        ) else {
            return Err(CandidateProjectionFailure::new(
                CANDIDATE_LAYOUT_UNKNOWN,
                "layout_kind",
                format!("layout_id={}", layout.id),
            ));
        };
        let mut matches = match self
            .evaluate_target_all(anchor, layout.suppress_iou_milli.unwrap_or(0))
            .map_err(|error| CandidateProjectionFailure::feature(anchor, error))?
        {
            TargetSearch::Complete(matches) => matches,
            TargetSearch::Incomplete { found, timeout_ms } => {
                return Err(CandidateProjectionFailure::search_incomplete(
                    found, timeout_ms,
                ));
            }
        };
        if matches.len() > usize::try_from(max_instances).unwrap_or(usize::MAX) {
            return Err(CandidateProjectionError::budget_exceeded(
                "candidates",
                format!(
                    "layout_id={} count={} limit_count={max_instances}",
                    layout.id,
                    matches.len()
                ),
            )
            .into());
        }
        match order {
            CandidateOrder::TopToBottom => matches.sort_by_key(|matched| (matched.y, matched.x)),
            CandidateOrder::LeftToRight => matches.sort_by_key(|matched| (matched.x, matched.y)),
        }
        let frame = PackRect {
            x: 0,
            y: 0,
            width: i32::try_from(self.scene.width()).unwrap_or(i32::MAX),
            height: i32::try_from(self.scene.height()).unwrap_or(i32::MAX),
        };
        let band = layout.readable_band.unwrap_or(frame);
        matches
            .iter()
            .map(|matched| {
                let origin = PackPoint {
                    x: matched.x,
                    y: matched.y,
                };
                let (Some(rect), Some(click)) = (
                    offset_rect(origin, instance_rect),
                    offset_rect(origin, click),
                ) else {
                    return Err(CandidateProjectionFailure::new(
                        INVALID_CANDIDATE_PROJECTION,
                        "rect",
                        format!("layout_id={} x={} y={}", layout.id, matched.x, matched.y),
                    ));
                };
                Ok(LayoutInstance {
                    rect: candidate_rect(rect),
                    click: candidate_rect(click),
                    actionable: rect_is_within(rect, band) && rect_is_within(click, band),
                    origin,
                    slot: None,
                })
            })
            .collect()
    }

    /// One evaluation of `target_id` with its region moved so that its top-left corner lies at
    /// `origin`, keeping its own size, then shifted by the sample's jitter. A template is
    /// evaluated through [`RecognitionEvaluator::evaluate_template_regions`].
    fn evaluate_placed(
        &self,
        target_id: &str,
        origin: PackPoint,
        sample: CandidateSampleVariant,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let evaluator = self.evaluator;
        let target = evaluator.target(target_id)?;
        let provider_ms = match target {
            RecognitionTarget::Ocr(target) => target.timeout_ms,
            _ => 0,
        };
        if self.sample_deadline.is_some_and(|deadline| {
            Instant::now() >= deadline
                || Duration::from_millis(provider_ms)
                    > deadline.saturating_duration_since(Instant::now())
        }) {
            return Err(RecognitionPackError::fatal(
                "recognition sample budget insufficient before backend evaluation",
            ));
        }
        let placed = sample.target(&placed_target(target, origin)?)?;
        match &placed {
            RecognitionTarget::Template(moved) => {
                let PackRegion::Rect(region) = &moved.region else {
                    return Err(RecognitionPackError::fatal(format!(
                        "template target '{target_id}' read at an offset has no rectangle region"
                    )));
                };
                let overridden;
                let evaluator = match sample.template_metric {
                    Some(metric) if metric != evaluator.pack.defaults.match_metric => {
                        let mut copy = evaluator.clone();
                        copy.pack.defaults.match_metric = metric;
                        overridden = copy;
                        &overridden
                    }
                    _ => evaluator,
                };
                let row = evaluator
                    .evaluate_template_regions(self.scene, target_id, &[*region])?
                    .rows
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        RecognitionPackError::fatal(format!(
                            "template target '{target_id}' region evaluation returned no row"
                        ))
                    })?;
                let template_ok = row.normalized_score >= row.threshold;
                let color_ok = row
                    .color
                    .is_none_or(|color| color.distance <= color.max_distance);
                Ok(TargetEvaluation {
                    id: target_id.to_owned(),
                    kind: TargetKind::Template,
                    passed: row.passed,
                    template: Some(TemplateEvaluation {
                        x: row.matched_rect.x,
                        y: row.matched_rect.y,
                        width: row.matched_rect.width,
                        height: row.matched_rect.height,
                        raw_score: row.raw_score,
                        score: row.normalized_score,
                        threshold: row.threshold,
                    }),
                    color: row.color,
                    ocr: None,
                    nn: None,
                    color_digest: None,
                    composite: None,
                    message: template_message(template_ok, color_ok),
                    sampling: None,
                    sample_evaluations: Vec::new(),
                })
            }
            RecognitionTarget::Ocr(moved) => evaluator.evaluate_ocr(self, moved),
            RecognitionTarget::Color(moved) => evaluator.evaluate_color(self.scene, moved),
            RecognitionTarget::ColorDigest(moved) => {
                evaluator.evaluate_color_digest(self.scene, moved)
            }
            _ => Err(RecognitionPackError::fatal(format!(
                "target '{target_id}' cannot be read at an offset"
            ))),
        }
    }
}

#[cfg(test)]
impl SceneEvaluation<'_> {
    /// The clock the anchor search reads its time limit from, for tests.
    pub(crate) fn with_search_clock(mut self, clock: fn() -> Instant) -> Self {
        self.search_clock = clock;
        self
    }
}

/// One instance of a projection: its rectangles, whether it is actionable, the origin its
/// offset features are read from, and its slot (`fixed_slots` only).
struct LayoutInstance<'l> {
    rect: CandidateRect,
    click: CandidateRect,
    actionable: bool,
    origin: PackPoint,
    slot: Option<&'l CandidateSlot>,
}

/// `rect`, relative to `origin`, in frame coordinates; `None` when a coordinate overflows.
fn offset_rect(origin: PackPoint, rect: PackRect) -> Option<PackRect> {
    let x = origin.x.checked_add(rect.x)?;
    let y = origin.y.checked_add(rect.y)?;
    x.checked_add(rect.width)?;
    y.checked_add(rect.height)?;
    Some(PackRect {
        x,
        y,
        width: rect.width,
        height: rect.height,
    })
}

/// `target` with its rectangle region moved so that its top-left corner lies at `origin`,
/// keeping its own size. Only template, OCR, color and color digest targets with a rectangle
/// region move, and a template's color check must be relative to its match.
fn placed_target(
    target: &RecognitionTarget,
    origin: PackPoint,
) -> RecognitionPackResult<RecognitionTarget> {
    let refused = || {
        RecognitionPackError::fatal(format!(
            "target '{}' cannot be read at an offset",
            target.id()
        ))
    };
    let place = |rect: &mut PackRect| {
        rect.x = origin.x;
        rect.y = origin.y;
    };
    let mut placed = target.clone();
    match &mut placed {
        RecognitionTarget::Template(template) => {
            if template
                .color_check
                .as_ref()
                .is_some_and(|check| !matches!(check.region, PackRegion::TemplateRelative(_)))
            {
                return Err(refused());
            }
            match &mut template.region {
                PackRegion::Rect(rect) => place(rect),
                _ => return Err(refused()),
            }
        }
        RecognitionTarget::Ocr(ocr) => match &mut ocr.region {
            PackRegion::Rect(rect) => place(rect),
            _ => return Err(refused()),
        },
        RecognitionTarget::Color(color) => place(&mut color.region),
        RecognitionTarget::ColorDigest(digest) => place(&mut digest.region),
        _ => return Err(refused()),
    }
    Ok(placed)
}

/// A sample's frame, jitter, effective template metric and target, and for a target read at
/// an offset the top-left corner it is placed at.
type SampleKey = (u8, i16, i16, u8, String, Option<(i32, i32)>);

fn sample_key(
    sample: CandidateSampleVariant,
    target: &str,
    placement: Option<PackPoint>,
) -> SampleKey {
    (
        sample.frame,
        sample.dx,
        sample.dy,
        match sample.template_metric {
            None => 0,
            Some(crate::RecognitionMatchMetric::CcorrNormed) => 1,
            Some(crate::RecognitionMatchMetric::CcoeffNormed) => 2,
        },
        target.to_owned(),
        placement.map(|point| (point.x, point.y)),
    )
}

impl CandidateLayout {
    /// One privacy per candidate: the strictest privacy of the targets its features read. A
    /// `fixed_slots` candidate reads its slot's targets, so the result has one entry per slot;
    /// every instance of a `repeated_anchor` layout reads the same targets, so the result
    /// repeats one privacy for each of the projection's `candidates`.
    pub fn candidate_privacy(
        &self,
        metadata: &actingcommand_contract::page_projection::VerifiedProjectionMetadata,
        candidates: usize,
    ) -> Vec<actingcommand_contract::page_projection::Privacy> {
        let placed = self
            .features
            .iter()
            .filter_map(|feature| feature.target.as_deref());
        match self.kind {
            CandidateLayoutKind::FixedSlots => self
                .slots
                .iter()
                .map(|slot| {
                    strictest_privacy(
                        metadata,
                        slot.targets
                            .values()
                            .map(String::as_str)
                            .chain(
                                slot.identity_templates
                                    .values()
                                    .flatten()
                                    .map(|template| template.target_id.as_str()),
                            )
                            .chain(placed.clone()),
                    )
                })
                .collect(),
            CandidateLayoutKind::RepeatedAnchor => {
                vec![strictest_privacy(metadata, placed); candidates]
            }
        }
    }

    /// Where `feature` reads its own target for an instance at `origin`: `origin` plus the
    /// feature's offset; `None` for a feature without one.
    fn feature_placement(
        &self,
        feature: &CandidateFeatureDeclaration,
        origin: PackPoint,
    ) -> Result<Option<PackPoint>, CandidateProjectionFailure> {
        let (Some(_), Some(offset)) = (&feature.target, feature.offset) else {
            return Ok(None);
        };
        match origin
            .x
            .checked_add(offset.x)
            .zip(origin.y.checked_add(offset.y))
        {
            Some((x, y)) => Ok(Some(PackPoint { x, y })),
            None => Err(CandidateProjectionFailure::new(
                INVALID_CANDIDATE_PROJECTION,
                "features",
                format!("layout_id={} feature={}", self.id, feature.name),
            )),
        }
    }

    pub fn required_frames(&self) -> usize {
        self.features
            .iter()
            .filter_map(|feature| feature.consensus.as_ref())
            .flat_map(|consensus| &consensus.samples)
            .map(|sample| usize::from(sample.frame) + 1)
            .max()
            .unwrap_or(1)
    }

    fn feature_samples(
        &self,
        feature: &CandidateFeatureDeclaration,
    ) -> Vec<CandidateSampleVariant> {
        feature
            .consensus
            .as_ref()
            .map(|consensus| consensus.samples.clone())
            .unwrap_or_else(|| {
                vec![CandidateSampleVariant {
                    frame: (self.required_frames() - 1) as u8,
                    ..CandidateSampleVariant::default()
                }]
            })
    }

    pub fn has_consensus(&self) -> bool {
        self.features
            .iter()
            .any(|feature| feature.consensus.is_some())
    }

    /// Declared worst provider time plus waits. Capture/CPU work still uses the original
    /// task and step deadline; this admission lower bound never allocates another budget. A
    /// `repeated_anchor` layout counts its per-instance provider time once for each instance
    /// the provider-evaluation budget lets one projection evaluate, at most `max_instances`.
    pub fn maximum_provider_and_wait_ms(&self, evaluator: &RecognitionEvaluator) -> u64 {
        let provider = sample_calls(self, evaluator.pack.defaults.match_metric)
            .keys()
            .map(|key| match evaluator.target(&key.4).ok() {
                Some(RecognitionTarget::Ocr(target)) => target.timeout_ms,
                Some(RecognitionTarget::Nn(target)) => target.timeout_ms,
                Some(RecognitionTarget::Composite(target)) => target
                    .members
                    .iter()
                    .map(|id| match evaluator.target(id).ok() {
                        Some(RecognitionTarget::Ocr(target)) => target.timeout_ms,
                        Some(RecognitionTarget::Nn(target)) => target.timeout_ms,
                        _ => 0,
                    })
                    .sum(),
                _ => 0,
            })
            .sum::<u64>();
        let instances = match self.kind {
            CandidateLayoutKind::FixedSlots => 1,
            CandidateLayoutKind::RepeatedAnchor => {
                let calls =
                    provider_evaluations(self, evaluator.pack.defaults.match_metric, |target_id| {
                        evaluator.target(target_id).ok()
                    });
                let budgeted = CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS
                    .checked_div(calls)
                    .unwrap_or(0);
                u64::from(self.max_instances.unwrap_or(0))
                    .min(u64::try_from(budgeted).unwrap_or(u64::MAX))
            }
        };
        provider.saturating_mul(instances)
            + (self.required_frames().saturating_sub(1) as u64) * u64::from(self.sample_interval_ms)
    }
}

/// The distinct backend evaluations one projection of `layout` performs; for a
/// `repeated_anchor` layout, those of one instance, which every actionable instance repeats.
fn sample_calls(
    layout: &CandidateLayout,
    default_metric: crate::RecognitionMatchMetric,
) -> BTreeMap<SampleKey, CandidateSampleVariant> {
    let origins = match layout.kind {
        CandidateLayoutKind::FixedSlots => layout
            .slots
            .iter()
            .map(|slot| {
                (
                    PackPoint {
                        x: slot.rect.x,
                        y: slot.rect.y,
                    },
                    Some(slot),
                )
            })
            .collect::<Vec<_>>(),
        CandidateLayoutKind::RepeatedAnchor => vec![(PackPoint { x: 0, y: 0 }, None)],
    };
    let mut calls = BTreeMap::new();
    for (origin, slot) in origins {
        for feature in &layout.features {
            for sample in layout.feature_samples(feature) {
                if let (Some(target), Some(offset)) = (&feature.target, feature.offset) {
                    let placement = PackPoint {
                        x: origin.x.saturating_add(offset.x),
                        y: origin.y.saturating_add(offset.y),
                    };
                    calls.insert(
                        sample_key(sample.effective(default_metric), target, Some(placement)),
                        sample,
                    );
                    continue;
                }
                let Some(slot) = slot else {
                    continue;
                };
                if let Some(target) = slot.targets.get(&feature.name) {
                    calls.insert(
                        sample_key(sample.effective(default_metric), target, None),
                        sample,
                    );
                }
                if let Some(templates) = slot.identity_templates.get(&feature.name) {
                    for template in templates {
                        calls.insert(
                            sample_key(sample.effective(default_metric), &template.target_id, None),
                            sample,
                        );
                    }
                }
            }
        }
    }
    calls
}

/// `Personal` when any of `targets` is not public, otherwise `Public`.
fn strictest_privacy<'t>(
    metadata: &actingcommand_contract::page_projection::VerifiedProjectionMetadata,
    targets: impl IntoIterator<Item = &'t str>,
) -> actingcommand_contract::page_projection::Privacy {
    use actingcommand_contract::page_projection::Privacy;
    if targets
        .into_iter()
        .any(|target| metadata.target_privacy(target) != Some(Privacy::Public))
    {
        Privacy::Personal
    } else {
        Privacy::Public
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
    feature: &CandidateFeatureDeclaration,
    evaluation: &TargetEvaluation,
) -> Result<Option<CandidateFeature>, CandidateProjectionFailure> {
    let confidence = confidence_milli(evaluation)?;
    Ok(match feature.value {
        CandidateFeatureValue::Passed => Some(CandidateFeature::Boolean {
            value: evaluation.passed,
            confidence,
        }),
        CandidateFeatureValue::MeasureMilli => {
            measure_milli(evaluation)?.map(|value| CandidateFeature::Integer { value, confidence })
        }
        CandidateFeatureValue::Identity => {
            let identity = feature.identity.as_ref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(evaluation, "has no identity declaration")
            })?;
            let ocr = evaluation.ocr.as_deref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(
                    evaluation,
                    "has no OCR text for its identity",
                )
            })?;
            Some(identity.map_ocr(
                &ocr.text,
                actingcommand_contract::ocr_confidence_milli(ocr.confidence),
            ))
        }
        CandidateFeatureValue::OcrInteger => {
            let integer = feature.integer.as_ref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(evaluation, "has no integer declaration")
            })?;
            let ocr = evaluation.ocr.as_deref().ok_or_else(|| {
                CandidateProjectionFailure::unmeasurable(evaluation, "has no integer OCR evidence")
            })?;
            Some(
                if confidence
                    .is_none_or(|value| value < i64::from(integer.minimum_confidence_milli))
                {
                    CandidateFeature::Unknown {
                        reason: CandidateUnknownReason::LowConfidence,
                        source: None,
                    }
                } else {
                    match integer
                        .format
                        .parse(ocr.text.trim(), integer.min, integer.max)
                    {
                        Ok(value) => CandidateFeature::Integer {
                            value: value as i64,
                            confidence,
                        },
                        Err(_) => CandidateFeature::Unknown {
                            reason: CandidateUnknownReason::OutOfDomain,
                            source: None,
                        },
                    }
                },
            )
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
/// For a `repeated_anchor` layout, the evaluations of one instance.
fn provider_evaluations<'t>(
    layout: &CandidateLayout,
    default_metric: crate::RecognitionMatchMetric,
    target: impl Fn(&str) -> Option<&'t RecognitionTarget>,
) -> usize {
    let is_provider = |target_id: &str| {
        matches!(
            target(target_id),
            Some(RecognitionTarget::Ocr(_) | RecognitionTarget::Nn(_))
        )
    };
    sample_calls(layout, default_metric)
        .into_keys()
        .map(|key| match target(&key.4) {
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

/// The wire shape of the schema `0.7` top-level `candidate_layouts`: unknown fields, a field
/// of the other layout kind, wrong JSON types, and a layout kind, order or feature value this
/// runtime does not know are refused with the pointer of the offending field.
pub(crate) fn validate_candidate_layouts_wire(value: &Value) -> RecognitionPackResult<()> {
    let layouts = wire_array(value, "/candidate_layouts")?;
    for (index, layout) in layouts.iter().enumerate() {
        let pointer = format!("/candidate_layouts/{index}");
        let layout = wire_object(layout, LAYOUT_FIELDS, &pointer)?;
        match layout.get("kind") {
            Some(kind) if kind.as_str() == Some("fixed_slots") => {
                reject_unknown_fields(layout, FIXED_SLOTS_FIELDS, &pointer)?
            }
            Some(kind) if kind.as_str() == Some("repeated_anchor") => {
                reject_unknown_fields(layout, REPEATED_ANCHOR_FIELDS, &pointer)?
            }
            Some(kind) => {
                return Err(declaration(
                    format!("{pointer}/kind"),
                    ResourceDeclarationReason::InvalidValue,
                    format!(
                        "{pointer}/kind {kind} is not a layout kind this runtime projects; expected \"fixed_slots\" or \"repeated_anchor\""
                    ),
                ));
            }
            None => {}
        }
        if let Some(order) = layout.get("order")
            && !matches!(order.as_str(), Some("top_to_bottom" | "left_to_right"))
        {
            return Err(declaration(
                format!("{pointer}/order"),
                ResourceDeclarationReason::InvalidValue,
                format!("{pointer}/order {order} is not top_to_bottom or left_to_right"),
            ));
        }
        for field in ["instance_rect", "click", "readable_band"] {
            if let Some(rect) = layout.get(field) {
                wire_object(
                    rect,
                    &["x", "y", "width", "height"],
                    &format!("{pointer}/{field}"),
                )?;
            }
        }
        if let Some(features) = layout.get("features") {
            let features_pointer = format!("{pointer}/features");
            for (feature_index, feature) in
                wire_array(features, &features_pointer)?.iter().enumerate()
            {
                let feature_pointer = format!("{features_pointer}/{feature_index}");
                let feature = wire_object(feature, FEATURE_FIELDS, &feature_pointer)?;
                if let Some(offset) = feature.get("offset") {
                    wire_object(offset, &["x", "y"], &format!("{feature_pointer}/offset"))?;
                }
                if let Some(value) = feature.get("value")
                    && !matches!(
                        value.as_str(),
                        Some("passed" | "measure_milli" | "identity" | "ocr_integer")
                    )
                {
                    return Err(declaration(
                        format!("{feature_pointer}/value"),
                        ResourceDeclarationReason::InvalidValue,
                        format!(
                            "{feature_pointer}/value {value} is not passed, measure_milli, identity or ocr_integer"
                        ),
                    ));
                }
            }
        }
        if let Some(slots) = layout.get("slots") {
            let slots_pointer = format!("{pointer}/slots");
            for (slot_index, slot) in wire_array(slots, &slots_pointer)?.iter().enumerate() {
                let slot_pointer = format!("{slots_pointer}/{slot_index}");
                let slot = wire_object(
                    slot,
                    &["rect", "click", "targets", "identity_templates"],
                    &slot_pointer,
                )?;
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
/// coordinate space, feature targets that exist and can be evaluated, the fields of each
/// layout kind, and feature regions read at an offset inside their slot or instance
/// rectangle. A layout's page is checked where the page set is loaded
/// ([`RecognitionPack::validate_candidate_layout_pages`]).
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
        let frame_count = layout.required_frames();
        if frame_count > 5
            || (frame_count > 1 && !(1..=1000).contains(&layout.sample_interval_ms))
            || (frame_count == 1 && layout.sample_interval_ms != 0)
        {
            errors.push(format!("{pointer}: multiple frames require sample_interval_ms 1..=1000, a single frame requires 0"));
        }
        let used = layout
            .features
            .iter()
            .flat_map(|feature| layout.feature_samples(feature))
            .map(|sample| usize::from(sample.frame))
            .collect::<BTreeSet<_>>();
        if used != (0..frame_count).collect() {
            errors.push(format!(
                "{pointer}: every captured frame must be consumed by a declared sample"
            ));
        }
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
        if !(1..=CANDIDATE_PROJECTION_MAX_FEATURES).contains(&layout.features.len()) {
            errors.push(format!(
                "{pointer}/features must declare 1..={CANDIDATE_PROJECTION_MAX_FEATURES} features, got {}",
                layout.features.len()
            ));
        }
        let mut features = HashMap::new();
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if (feature.value == CandidateFeatureValue::OcrInteger) != feature.integer.is_some() {
                errors.push(format!("{pointer}/features/{feature_index}: ocr_integer value and integer declaration must appear together"));
            }
            if let Some(integer) = &feature.integer
                && let Err(error) = integer.validate()
            {
                errors.push(format!(
                    "{pointer}/features/{feature_index}/integer: {error}"
                ));
            }
            if let Some(consensus) = &feature.consensus
                && let Err(error) = consensus.validate(feature.value)
            {
                errors.push(format!(
                    "{pointer}/features/{feature_index}/consensus: {error}"
                ));
            }
            if (feature.value == CandidateFeatureValue::Identity) != feature.identity.is_some() {
                errors.push(format!("{pointer}/features/{feature_index}: identity value and declaration must appear together"));
            }
            if let Some(identity) = &feature.identity
                && let Err(error) = identity.validate()
            {
                errors.push(format!(
                    "{pointer}/features/{feature_index}/identity: {error}"
                ));
            }
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
            let feature_pointer = format!("{pointer}/features/{feature_index}");
            match (&feature.target, feature.offset) {
                (None, None) => {}
                (Some(target_id), Some(_)) => validate_placed_feature(
                    pack,
                    &targets,
                    feature,
                    target_id,
                    &feature_pointer,
                    errors,
                ),
                _ => errors.push(format!(
                    "{feature_pointer}: target and offset must appear together"
                )),
            }
        }
        match layout.kind {
            CandidateLayoutKind::FixedSlots => {
                validate_fixed_slots_fields(layout, &pointer, errors)
            }
            CandidateLayoutKind::RepeatedAnchor => {
                validate_repeated_anchor(pack, &targets, layout, &pointer, errors)
            }
        }
        if layout.kind == CandidateLayoutKind::FixedSlots
            && !(1..=CANDIDATE_PROJECTION_MAX_CANDIDATES).contains(&layout.slots.len())
        {
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
                if layout
                    .features
                    .iter()
                    .any(|feature| feature.name == *name && feature.target.is_some())
                {
                    errors.push(format!(
                        "{target_pointer}: feature '{name}' reads its own target at an offset"
                    ));
                }
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
                    Some(target) if matches!(*value, CandidateFeatureValue::Identity | CandidateFeatureValue::OcrInteger) && !matches!(target, RecognitionTarget::Ocr(_)) => {
                        errors.push(format!("{target_pointer}: identity target must be OCR; icon identities use identity_templates"));
                    }
                    Some(_) => {}
                }
            }
            for feature in &layout.features {
                if let Some(consensus) = &feature.consensus {
                    let ids = slot
                        .targets
                        .get(&feature.name)
                        .into_iter()
                        .map(String::as_str)
                        .chain(
                            slot.identity_templates
                                .get(&feature.name)
                                .into_iter()
                                .flatten()
                                .map(|template| template.target_id.as_str()),
                        );
                    for id in ids {
                        if let Some(target) = targets.get(id) {
                            crate::target_consensus::validate_sample_variants(
                                pack,
                                target,
                                &consensus.samples,
                                errors,
                            );
                            for sample in &consensus.samples {
                                match sample.target(target) {
                                    Err(error) => errors.push(format!(
                                        "{slot_pointer}/{}: {}",
                                        feature.name,
                                        error.message()
                                    )),
                                    Ok(target) => {
                                        let region = match &target {
                                            RecognitionTarget::Ocr(target) => target.region.clone(),
                                            RecognitionTarget::Template(target) => {
                                                target.region.clone()
                                            }
                                            RecognitionTarget::Color(target) => {
                                                PackRegion::Rect(target.region)
                                            }
                                            RecognitionTarget::ColorDigest(target) => {
                                                PackRegion::Rect(target.region)
                                            }
                                            _ => continue,
                                        };
                                        validate_region_within_coordinate_space(
                                            &region,
                                            pack.coordinate_space,
                                            &slot_pointer,
                                            errors,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                let Some(identity) = &feature.identity else {
                    continue;
                };
                let pool = slot.identity_templates.get(&feature.name);
                match &identity.recognition {
                    CandidateIdentityRecognition::OcrAliases { .. } if pool.is_some() => errors
                        .push(format!(
                            "{slot_pointer}: OCR identity cannot carry an icon pool"
                        )),
                    CandidateIdentityRecognition::IconTemplates { .. } => {
                        if slot.targets.contains_key(&feature.name) {
                            errors.push(format!(
                                "{slot_pointer}: icon identity cannot also name an OCR target"
                            ));
                        }
                        let Some(pool) = pool else {
                            errors.push(format!(
                                "{slot_pointer}: icon identity has no template pool"
                            ));
                            continue;
                        };
                        if !(1..=crate::candidate_identity::MAX_IDENTITY_TEMPLATES)
                            .contains(&pool.len())
                        {
                            errors.push(format!(
                                "{slot_pointer}: icon pool must contain 1..=16 templates"
                            ));
                        }
                        let mut distinct = BTreeSet::new();
                        for template in pool {
                            if !distinct.insert(&template.target_id)
                                || !matches!(
                                    targets.get(template.target_id.as_str()),
                                    Some(RecognitionTarget::Template(_))
                                )
                                || !identity.entries.iter().any(|entry| {
                                    entry.id == template.id && entry.variant == template.variant
                                })
                            {
                                errors.push(format!("{slot_pointer}: icon pool has an invalid, duplicate or out-of-domain template '{}'", template.target_id));
                            }
                        }
                    }
                    _ => {}
                }
            }
            for feature in &layout.features {
                let origin = PackPoint {
                    x: slot.rect.x,
                    y: slot.rect.y,
                };
                if placed_regions(&targets, layout, feature)
                    .into_iter()
                    .filter_map(|region| offset_rect(origin, region))
                    .any(|region| !rect_is_within(region, slot.rect))
                {
                    errors.push(format!(
                        "{slot_pointer}: feature '{}' read at an offset lies outside the slot rect",
                        feature.name
                    ));
                }
            }
            for name in slot.identity_templates.keys() {
                if !layout
                    .features
                    .iter()
                    .any(|feature| feature.name == *name && feature.identity.is_some())
                {
                    errors.push(format!(
                        "{slot_pointer}/identity_templates/{name}: undeclared identity feature"
                    ));
                }
            }
            for id in slot.targets.values().chain(
                slot.identity_templates
                    .values()
                    .flatten()
                    .map(|template| &template.target_id),
            ) {
                if pack.target_consensus.contains_key(id)
                    || matches!(targets.get(id.as_str()), Some(RecognitionTarget::Composite(composite)) if composite.members.iter().any(|member| pack.target_consensus.contains_key(member)))
                {
                    errors.push(format!("{slot_pointer}: candidate feature '{id}' requires raw targets throughout its reference closure"));
                }
            }
        }
        let provider = provider_evaluations(layout, pack.defaults.match_metric, |target_id| {
            targets.get(target_id).copied()
        });
        if provider > CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS {
            let field = match layout.kind {
                CandidateLayoutKind::FixedSlots => "slots",
                CandidateLayoutKind::RepeatedAnchor => "features",
            };
            errors.push(format!(
                "{pointer}/{field}: {provider} OCR and NN evaluations exceed the {CANDIDATE_PROJECTION_MAX_PROVIDER_EVALUATIONS}-evaluation projection budget"
            ));
        }
    }
}

/// A feature that reads its own target at an offset: the target exists, it is a template, OCR,
/// color or color digest target with a rectangle region (`nn` and `composite` features cannot
/// move), a template's color check is relative to its match, an identity or integer feature
/// reads OCR, an icon identity reads its pool instead, and the target is raw.
fn validate_placed_feature(
    pack: &RecognitionPack,
    targets: &HashMap<&str, &RecognitionTarget>,
    feature: &CandidateFeatureDeclaration,
    target_id: &str,
    pointer: &str,
    errors: &mut Vec<String>,
) {
    let target_pointer = format!("{pointer}/target");
    let target = targets.get(target_id).copied();
    match target {
        None => errors.push(format!(
            "{target_pointer}: target '{target_id}' does not exist"
        )),
        Some(RecognitionTarget::Template(template)) => {
            if !matches!(template.region, PackRegion::Rect(_)) {
                errors.push(format!(
                    "{target_pointer}: template '{target_id}' read at an offset needs a rectangle region"
                ));
            }
            if template
                .color_check
                .as_ref()
                .is_some_and(|check| !matches!(check.region, PackRegion::TemplateRelative(_)))
            {
                errors.push(format!(
                    "{target_pointer}: template '{target_id}' read at an offset may declare only a template_relative color_check"
                ));
            }
        }
        Some(RecognitionTarget::Ocr(ocr)) => {
            if !matches!(ocr.region, PackRegion::Rect(_)) {
                errors.push(format!(
                    "{target_pointer}: OCR target '{target_id}' read at an offset needs a rectangle region"
                ));
            }
        }
        Some(RecognitionTarget::Color(_) | RecognitionTarget::ColorDigest(_)) => {}
        Some(
            RecognitionTarget::Nn(_)
            | RecognitionTarget::Composite(_)
            | RecognitionTarget::ClickOnly(_),
        ) => errors.push(format!(
            "{target_pointer}: target '{target_id}' cannot be read at an offset; such a feature reads a template, color, color_digest or ocr target"
        )),
    }
    if matches!(
        feature.value,
        CandidateFeatureValue::Identity | CandidateFeatureValue::OcrInteger
    ) && !matches!(target, Some(RecognitionTarget::Ocr(_)))
    {
        errors.push(format!(
            "{target_pointer}: identity target must be OCR; icon identities use identity_templates"
        ));
    }
    if feature.identity.as_ref().is_some_and(|identity| {
        matches!(
            identity.recognition,
            CandidateIdentityRecognition::IconTemplates { .. }
        )
    }) {
        errors.push(format!(
            "{pointer}/identity: an icon identity reads its template pool, not a target at an offset"
        ));
    }
    if pack.target_consensus.contains_key(target_id) {
        errors.push(format!(
            "{target_pointer}: candidate feature '{target_id}' requires raw targets throughout its reference closure"
        ));
    }
    if let (Some(consensus), Some(target)) = (&feature.consensus, target) {
        for sample in &consensus.samples {
            if let Err(error) = sample.target(target) {
                errors.push(format!("{pointer}/consensus: {}", error.message()));
            }
        }
    }
}

/// The rectangles a feature read at an offset covers, relative to its instance's origin: one
/// per sample, each with the target's own size, shifted by the sample's jitter. Empty for a
/// feature without an offset or whose target has no rectangle region.
fn placed_regions(
    targets: &HashMap<&str, &RecognitionTarget>,
    layout: &CandidateLayout,
    feature: &CandidateFeatureDeclaration,
) -> Vec<PackRect> {
    let (Some(target_id), Some(offset)) = (&feature.target, feature.offset) else {
        return Vec::new();
    };
    let size = match targets.get(target_id.as_str()).copied() {
        Some(RecognitionTarget::Template(template)) => match template.region {
            PackRegion::Rect(rect) => rect,
            _ => return Vec::new(),
        },
        Some(RecognitionTarget::Ocr(ocr)) => match ocr.region {
            PackRegion::Rect(rect) => rect,
            _ => return Vec::new(),
        },
        Some(RecognitionTarget::Color(color)) => color.region,
        Some(RecognitionTarget::ColorDigest(digest)) => digest.region,
        _ => return Vec::new(),
    };
    layout
        .feature_samples(feature)
        .into_iter()
        .map(|sample| PackRect {
            x: offset.x.saturating_add(i32::from(sample.dx)),
            y: offset.y.saturating_add(i32::from(sample.dy)),
            width: size.width,
            height: size.height,
        })
        .collect()
}

/// A `fixed_slots` layout declares none of the `repeated_anchor` fields.
fn validate_fixed_slots_fields(layout: &CandidateLayout, pointer: &str, errors: &mut Vec<String>) {
    for (field, declared) in [
        ("anchor", layout.anchor.is_some()),
        ("max_instances", layout.max_instances.is_some()),
        ("order", layout.order.is_some()),
        ("suppress_iou_milli", layout.suppress_iou_milli.is_some()),
        ("instance_rect", layout.instance_rect.is_some()),
        ("click", layout.click.is_some()),
        ("readable_band", layout.readable_band.is_some()),
    ] {
        if declared {
            errors.push(format!(
                "{pointer}/{field}: only a repeated_anchor layout declares it"
            ));
        }
    }
}

/// A `repeated_anchor` layout: a static template anchor, 1..=64 instances, an order, a
/// suppression of 0..=999 milli, non-empty instance and click rectangles, a readable band
/// inside the coordinate space, no slots, features that each read a target at an offset whose
/// regions lie inside the instance rectangle, and same-frame consensus only (a scan layout
/// cannot capture further frames, Workflow #308 S0 section 1.8).
fn validate_repeated_anchor(
    pack: &RecognitionPack,
    targets: &HashMap<&str, &RecognitionTarget>,
    layout: &CandidateLayout,
    pointer: &str,
    errors: &mut Vec<String>,
) {
    if !layout.slots.is_empty() {
        errors.push(format!(
            "{pointer}/slots: a repeated_anchor layout declares no slots"
        ));
    }
    let anchor_pointer = format!("{pointer}/anchor");
    match layout.anchor.as_deref() {
        None => errors.push(format!("{anchor_pointer}: the layout names no anchor")),
        Some(anchor) => {
            match targets.get(anchor).copied() {
                Some(RecognitionTarget::Template(template)) => {
                    if matches!(template.region, PackRegion::TemplateRelative(_)) {
                        errors.push(format!(
                            "{anchor_pointer}: the search region must be static"
                        ));
                    }
                    if template.color_check.as_ref().is_some_and(|check| {
                        !matches!(check.region, PackRegion::TemplateRelative(_))
                    }) {
                        errors.push(format!(
                            "{anchor_pointer}: its color_check must be template_relative"
                        ));
                    }
                    if pack.target_consensus.contains_key(anchor) {
                        errors.push(format!("{anchor_pointer}: '{anchor}' must be a raw target"));
                    }
                }
                _ => errors.push(format!(
                    "{anchor_pointer}: '{anchor}' is not a template target"
                )),
            }
        }
    }
    if !layout.max_instances.is_some_and(|max| {
        usize::try_from(max)
            .is_ok_and(|max| (1..=CANDIDATE_PROJECTION_MAX_CANDIDATES).contains(&max))
    }) {
        errors.push(format!(
            "{pointer}/max_instances must be 1..={CANDIDATE_PROJECTION_MAX_CANDIDATES}"
        ));
    }
    if layout.order.is_none() {
        errors.push(format!(
            "{pointer}/order: a repeated_anchor layout declares top_to_bottom or left_to_right"
        ));
    }
    if layout
        .suppress_iou_milli
        .is_some_and(|milli| milli > MAX_SUPPRESS_IOU_MILLI)
    {
        errors.push(format!(
            "{pointer}/suppress_iou_milli must be 0..={MAX_SUPPRESS_IOU_MILLI}"
        ));
    }
    for (field, rect) in [
        ("instance_rect", layout.instance_rect),
        ("click", layout.click),
    ] {
        match rect {
            None => errors.push(format!(
                "{pointer}/{field}: a repeated_anchor layout declares it"
            )),
            Some(rect) if rect.width <= 0 || rect.height <= 0 => errors.push(format!(
                "{pointer}/{field} dimensions must be positive: {}x{}",
                rect.width, rect.height
            )),
            Some(_) => {}
        }
    }
    if let Some(band) = layout.readable_band {
        let label = format!("{pointer}/readable_band");
        validate_rect_shape(band, &label, errors);
        validate_region_within_coordinate_space(
            &PackRegion::Rect(band),
            pack.coordinate_space,
            &label,
            errors,
        );
    }
    for (feature_index, feature) in layout.features.iter().enumerate() {
        let feature_pointer = format!("{pointer}/features/{feature_index}");
        if feature.target.is_none() || feature.offset.is_none() {
            errors.push(format!(
                "{feature_pointer}: a repeated_anchor feature reads a target at an offset"
            ));
        }
        if feature
            .consensus
            .as_ref()
            .is_some_and(|consensus| consensus.samples.iter().any(|sample| sample.frame != 0))
        {
            errors.push(format!(
                "{feature_pointer}/consensus: a repeated_anchor layout samples one frame; consensus over more than one frame is refused"
            ));
        }
        if let Some(instance_rect) = layout.instance_rect
            && placed_regions(targets, layout, feature)
                .into_iter()
                .any(|region| !rect_is_within(region, instance_rect))
        {
            errors.push(format!(
                "{feature_pointer}/offset: the feature region lies outside instance_rect"
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
