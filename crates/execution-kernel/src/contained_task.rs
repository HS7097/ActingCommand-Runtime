// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned admission and execution for contained semantic task packages.

use crate::{
    ExecutionBundleError, ExternalExpectedSha256, ExternallyVerifiedBundle, RunDirective,
    RunFailureObservation, RunFailureStage, RunOperationCandidate, RunOperationFailureDecision,
    RunOperationPolicy, RunStateConfig, RunStateMachine, RunTerminal, decide_run_operation_failure,
    select_run_operation,
};
use actingcommand_contract::{
    InputAction, InputSamplingEvidence, InputSamplingRegion, SchedulingEffectCondition,
    SchedulingOutcomeDeclaration, TaskOutcome,
};
use actingcommand_device::{Frame, PixelFormat};
use actingcommand_pack_containment::{ContainmentError, LoadedBundle, Sha256Hash};
use actingcommand_page_detector::{PageDetector, PageSet};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    OcrProviderExecutionEvidence, OcrTextEvidence, PackRegion, RecognitionEvaluator,
    RecognitionPackErrorCode, RecognitionTarget, TargetEvaluation, TargetKind, VisionProvider,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 50;
const DEFAULT_TASK_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_STEPS: u32 = 100;
const MAX_TASK_TIMEOUT_MS: u64 = 600_000;
const MAX_STEP_TIMEOUT_MS: u64 = 60_000;
const MAX_CAPTURE_INTERVAL_MS: u64 = 5_000;
const MAX_STEPS: u32 = 1_000;
const MAX_STABILITY_PIXEL_BYTES: usize = 4;
const MAX_POST_ADMISSION_OCR_FRAMES: u32 = 256;
const MAX_POST_ADMISSION_OCR_ITEMS: u32 = 4_096;
const MAX_POST_ADMISSION_OCR_STRING_BYTES: u32 = 4_096;
const MAX_POST_ADMISSION_OCR_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_POST_ADMISSION_OCR_TRUTH_ENTRIES: u32 = 4_096;
const MAX_POST_ADMISSION_OCR_TARGETS: usize = 32;
const POST_ADMISSION_OCR_TRUTH_SCHEMA: &str = "actingcommand.ocr-truth-set.v1";

fn deserialize_non_null_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedTaskError {
    code: &'static str,
    detail: Option<String>,
}

impl ContainedTaskError {
    fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for ContainedTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "contained task error {}", self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for ContainedTaskError {}

#[derive(Debug)]
pub enum ContainedTaskRunError<E> {
    Boundary(E),
    Task(ContainedTaskError),
}

impl<E> From<ContainedTaskError> for ContainedTaskRunError<E> {
    fn from(error: ContainedTaskError) -> Self {
        Self::Task(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StabilityRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilityComparisonMode {
    ExactPixelsV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StabilityComparisonParameters {}

impl<'de> Deserialize<'de> for StabilityComparisonParameters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EmptyObjectVisitor;

        impl<'de> serde::de::Visitor<'de> for EmptyObjectVisitor {
            type Value = StabilityComparisonParameters;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an empty exact_pixels_v1 parameters object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                if let Some(key) = map.next_key::<String>()? {
                    let _: serde::de::IgnoredAny = map.next_value()?;
                    return Err(serde::de::Error::unknown_field(&key, &[]));
                }
                Ok(StabilityComparisonParameters {})
            }
        }

        deserializer.deserialize_map(EmptyObjectVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StabilityComparisonDeclaration {
    pub mode: StabilityComparisonMode,
    pub parameters: StabilityComparisonParameters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StabilityTerminationDeclaration {
    pub region: StabilityRegion,
    pub comparison: StabilityComparisonDeclaration,
    pub consecutive_unchanged_threshold: u32,
    pub max_steps: u32,
}

impl StabilityTerminationDeclaration {
    fn validate(
        &self,
        resolution: &Resolution,
        root_max_steps: Option<u32>,
    ) -> Result<(), ContainedTaskError> {
        if root_max_steps != Some(self.max_steps)
            || self.consecutive_unchanged_threshold == 0
            || self.consecutive_unchanged_threshold >= self.max_steps
            || self.max_steps > MAX_STEPS
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        self.region.validate(resolution)
    }
}

impl StabilityRegion {
    fn validate(&self, resolution: &Resolution) -> Result<(), ContainedTaskError> {
        let Some(end_x) = self.x.checked_add(self.width) else {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        };
        let Some(end_y) = self.y.checked_add(self.height) else {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        };
        if self.width == 0
            || self.height == 0
            || end_x > resolution.width
            || end_y > resolution.height
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        validate_stability_crop_byte_layout(*self, resolution)
    }
}

fn validate_stability_crop_byte_layout(
    region: StabilityRegion,
    resolution: &Resolution,
) -> Result<(), ContainedTaskError> {
    let invalid = || ContainedTaskError::new("contained_task_control_invalid");
    let frame_width = usize::try_from(resolution.width).map_err(|_| invalid())?;
    let frame_height = usize::try_from(resolution.height).map_err(|_| invalid())?;
    let x = usize::try_from(region.x).map_err(|_| invalid())?;
    let y = usize::try_from(region.y).map_err(|_| invalid())?;
    let width = usize::try_from(region.width).map_err(|_| invalid())?;
    let height = usize::try_from(region.height).map_err(|_| invalid())?;
    let frame_stride = frame_width
        .checked_mul(MAX_STABILITY_PIXEL_BYTES)
        .ok_or_else(invalid)?;
    let frame_bytes = frame_stride.checked_mul(frame_height).ok_or_else(invalid)?;
    let row_bytes = width
        .checked_mul(MAX_STABILITY_PIXEL_BYTES)
        .ok_or_else(invalid)?;
    let last_row = y.checked_add(height - 1).ok_or_else(invalid)?;
    let last_row_start = last_row
        .checked_mul(frame_stride)
        .and_then(|offset| {
            x.checked_mul(MAX_STABILITY_PIXEL_BYTES)
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .ok_or_else(invalid)?;
    let crop_end = last_row_start.checked_add(row_bytes).ok_or_else(invalid)?;
    if crop_end > frame_bytes {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilityComparisonResult {
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilityTerminalReason {
    ConsecutiveUnchangedThresholdReached,
    MaxStepsReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostAdmissionOcrNormalization {
    TrimLowercaseV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostAdmissionOcrComparisonMode {
    ExactSetV1,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostAdmissionOcrObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<Option<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<Vec<OcrTextEvidence>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution: Option<OcrProviderExecutionEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Vec<PostAdmissionOcrTargetObservation>>,
}

// Observation confidences come only from the recognition owner's finite-score validation.
impl Eq for PostAdmissionOcrObservation {}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostAdmissionOcrTargetObservation {
    target_id: String,
    text: String,
    confidence: Option<f32>,
    blocks: Vec<OcrTextEvidence>,
    execution: OcrProviderExecutionEvidence,
}

// Target confidences come only from the recognition owner's finite-score validation.
impl Eq for PostAdmissionOcrTargetObservation {}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostAdmissionOcrObservedValue {
    value: String,
    occurrences: u32,
    confidences: Vec<Option<f32>>,
}

// Collected confidences inherit the same finite-score invariant as each observation.
impl Eq for PostAdmissionOcrObservedValue {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PostAdmissionOcrDuplicateEvidence {
    pub value: String,
    pub occurrences: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostAdmissionOcrComparisonReport {
    schema_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_ids: Option<Vec<String>>,
    truth_set_path: String,
    truth_set_sha256: String,
    normalization: PostAdmissionOcrNormalization,
    comparison: PostAdmissionOcrComparisonMode,
    outcome_key: String,
    frames_collected: u32,
    items_collected: u32,
    discarded_empty_items: u32,
    total_observed_utf8_bytes: u64,
    exact_match: bool,
    truth: Vec<String>,
    observed: Vec<PostAdmissionOcrObservedValue>,
    missed: Vec<String>,
    unexpected: Vec<String>,
    duplicates: Vec<PostAdmissionOcrDuplicateEvidence>,
}

impl Eq for PostAdmissionOcrComparisonReport {}

impl PostAdmissionOcrComparisonReport {
    pub fn outcome_key(&self) -> &str {
        &self.outcome_key
    }

    pub const fn frames_collected(&self) -> u32 {
        self.frames_collected
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrTruthReference {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrLimits {
    max_frames: u32,
    max_items: u32,
    max_string_bytes: u32,
    max_total_bytes: u64,
    max_truth_entries: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrDeclaration {
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    page_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    page_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    target_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    target_ids: Option<Vec<String>>,
    truth_set: PostAdmissionOcrTruthReference,
    normalization: PostAdmissionOcrNormalization,
    comparison: PostAdmissionOcrComparisonMode,
    limits: PostAdmissionOcrLimits,
    outcome_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrTruthSet {
    schema_version: String,
    items: Vec<String>,
}

#[derive(Debug, Clone)]
struct PreparedPostAdmissionOcr {
    declaration: PostAdmissionOcrDeclaration,
    page_ids: Vec<String>,
    target_ids: Vec<String>,
    truth: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PostAdmissionOcrObservedAggregate {
    occurrences: u32,
    confidences: Vec<Option<f32>>,
}

#[derive(Debug, Default)]
struct PostAdmissionOcrCollector<'a> {
    prepared: Option<&'a PreparedPostAdmissionOcr>,
    frames_collected: u32,
    items_collected: u32,
    discarded_empty_items: u32,
    total_observed_utf8_bytes: u64,
    values: BTreeMap<String, PostAdmissionOcrObservedAggregate>,
    invocation_ids: BTreeSet<String>,
    stream_binding: Option<OcrProviderExecutionEvidence>,
}

impl<'a> PostAdmissionOcrCollector<'a> {
    fn new(prepared: Option<&'a PreparedPostAdmissionOcr>) -> Self {
        Self {
            prepared,
            ..Self::default()
        }
    }

    fn observe(
        &mut self,
        game: &str,
        evaluator: &RecognitionEvaluator,
        page_label: &str,
        scene: &Scene,
    ) -> Result<Option<(u32, PostAdmissionOcrObservation)>, ContainedTaskError> {
        let Some(prepared) = self.prepared else {
            return Ok(None);
        };
        let declaration = &prepared.declaration;
        if !prepared
            .page_ids
            .iter()
            .any(|page_id| crate::page_anchor_matches(game, page_label, page_id))
            || self.frames_collected >= declaration.limits.max_frames
        {
            return Ok(None);
        }
        let mut invocation_ids = self.invocation_ids.clone();
        let mut stream_binding = self.stream_binding.clone();
        let mut values = self.values.clone();
        let mut discarded_empty_items = self.discarded_empty_items;
        let mut new_item_count = self.items_collected;
        let mut added_bytes = 0_u64;
        let max_string_bytes = usize::try_from(declaration.limits.max_string_bytes)
            .map_err(|_| ContainedTaskError::new("contained_task_post_admission_ocr_invalid"))?;
        let mut target_observations = Vec::with_capacity(prepared.target_ids.len());
        for target_id in &prepared.target_ids {
            let evaluated = evaluator
                .evaluate_ocr_observation(scene, target_id)
                .map_err(|error| {
                    ContainedTaskError::with_detail(
                        "contained_task_post_admission_ocr_failed",
                        error.to_string(),
                    )
                })?;
            if evaluated.target_id != *target_id
                || !invocation_ids.insert(evaluated.execution.invocation_id.clone())
                || stream_binding
                    .as_ref()
                    .is_some_and(|binding| !same_ocr_stream_binding(binding, &evaluated.execution))
            {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_evidence_mismatch",
                ));
            }
            if stream_binding.is_none() {
                stream_binding = Some(evaluated.execution.clone());
            }
            let block_count = u32::try_from(evaluated.blocks.len()).map_err(|_| {
                ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
            })?;
            new_item_count = new_item_count.checked_add(block_count).ok_or_else(|| {
                ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
            })?;
            if new_item_count > declaration.limits.max_items {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_limit_exceeded",
                ));
            }
            added_bytes = added_bytes
                .checked_add(u64::try_from(evaluated.text.len()).map_err(|_| {
                    ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
                })?)
                .ok_or_else(|| {
                    ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
                })?;
            for block in &evaluated.blocks {
                if block.text.len() > max_string_bytes {
                    return Err(ContainedTaskError::new(
                        "contained_task_post_admission_ocr_limit_exceeded",
                    ));
                }
                added_bytes = added_bytes
                    .checked_add(u64::try_from(block.text.len()).map_err(|_| {
                        ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
                    })?)
                    .ok_or_else(|| {
                        ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
                    })?;
                let value = normalize_post_admission_ocr(&block.text, declaration.normalization);
                if value.len() > max_string_bytes {
                    return Err(ContainedTaskError::new(
                        "contained_task_post_admission_ocr_limit_exceeded",
                    ));
                }
                if value.is_empty() {
                    discarded_empty_items =
                        discarded_empty_items.checked_add(1).ok_or_else(|| {
                            ContainedTaskError::new(
                                "contained_task_post_admission_ocr_limit_exceeded",
                            )
                        })?;
                    continue;
                }
                let aggregate = values.entry(value).or_default();
                aggregate.occurrences = aggregate.occurrences.checked_add(1).ok_or_else(|| {
                    ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
                })?;
                aggregate.confidences.push(block.confidence);
            }
            target_observations.push(PostAdmissionOcrTargetObservation {
                target_id: evaluated.target_id,
                text: evaluated.text,
                confidence: evaluated.confidence,
                blocks: evaluated.blocks,
                execution: evaluated.execution,
            });
        }
        let new_total_bytes = self
            .total_observed_utf8_bytes
            .checked_add(added_bytes)
            .ok_or_else(|| {
                ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
            })?;
        if new_total_bytes > declaration.limits.max_total_bytes {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_limit_exceeded",
            ));
        }
        let frame_index = self.frames_collected;
        let frames_collected = self.frames_collected.checked_add(1).ok_or_else(|| {
            ContainedTaskError::new("contained_task_post_admission_ocr_limit_exceeded")
        })?;
        self.invocation_ids = invocation_ids;
        self.stream_binding = stream_binding;
        self.values = values;
        self.discarded_empty_items = discarded_empty_items;
        self.frames_collected = frames_collected;
        self.items_collected = new_item_count;
        self.total_observed_utf8_bytes = new_total_bytes;
        let observation = if declaration.target_id.is_some() {
            let [observation] = target_observations.as_slice() else {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_evidence_mismatch",
                ));
            };
            PostAdmissionOcrObservation {
                target_id: Some(observation.target_id.clone()),
                text: Some(observation.text.clone()),
                confidence: Some(observation.confidence),
                blocks: Some(observation.blocks.clone()),
                execution: Some(observation.execution.clone()),
                targets: None,
            }
        } else {
            PostAdmissionOcrObservation {
                target_id: None,
                text: None,
                confidence: None,
                blocks: None,
                execution: None,
                targets: Some(target_observations),
            }
        };
        Ok(Some((frame_index, observation)))
    }

    fn finish(self) -> Result<Option<PostAdmissionOcrComparisonReport>, ContainedTaskError> {
        let Some(prepared) = self.prepared else {
            return Ok(None);
        };
        if self.frames_collected == 0 || self.stream_binding.is_none() {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_observation_missing",
            ));
        }
        let observed_set = self.values.keys().cloned().collect::<BTreeSet<_>>();
        let truth_set = prepared.truth.iter().cloned().collect::<BTreeSet<_>>();
        let missed = truth_set
            .difference(&observed_set)
            .cloned()
            .collect::<Vec<_>>();
        let unexpected = observed_set
            .difference(&truth_set)
            .cloned()
            .collect::<Vec<_>>();
        let observed = self
            .values
            .iter()
            .map(|(value, aggregate)| PostAdmissionOcrObservedValue {
                value: value.clone(),
                occurrences: aggregate.occurrences,
                confidences: aggregate.confidences.clone(),
            })
            .collect::<Vec<_>>();
        let duplicates = self
            .values
            .iter()
            .filter(|(_, aggregate)| aggregate.occurrences > 1)
            .map(|(value, aggregate)| PostAdmissionOcrDuplicateEvidence {
                value: value.clone(),
                occurrences: aggregate.occurrences,
            })
            .collect::<Vec<_>>();
        Ok(Some(PostAdmissionOcrComparisonReport {
            schema_version: "actingcommand.runtime.post-admission-ocr-comparison.v1",
            target_id: prepared.declaration.target_id.clone(),
            target_ids: prepared.declaration.target_ids.clone(),
            truth_set_path: prepared.declaration.truth_set.path.clone(),
            truth_set_sha256: prepared.declaration.truth_set.sha256.clone(),
            normalization: prepared.declaration.normalization,
            comparison: prepared.declaration.comparison,
            outcome_key: prepared.declaration.outcome_key.clone(),
            frames_collected: self.frames_collected,
            items_collected: self.items_collected,
            discarded_empty_items: self.discarded_empty_items,
            total_observed_utf8_bytes: self.total_observed_utf8_bytes,
            exact_match: missed.is_empty() && unexpected.is_empty(),
            truth: prepared.truth.clone(),
            observed,
            missed,
            unexpected,
            duplicates,
        }))
    }
}

fn normalize_post_admission_ocr(
    value: &str,
    normalization: PostAdmissionOcrNormalization,
) -> String {
    match normalization {
        PostAdmissionOcrNormalization::TrimLowercaseV1 => value.trim().to_lowercase(),
    }
}

fn same_ocr_stream_binding(
    left: &OcrProviderExecutionEvidence,
    right: &OcrProviderExecutionEvidence,
) -> bool {
    left.session_id == right.session_id
        && left.session_generation == right.session_generation
        && left.requested_provider == right.requested_provider
        && left.resolved_provider == right.resolved_provider
        && left.requested_cuda_ordinal == right.requested_cuda_ordinal
        && left.requested_cuda_identity == right.requested_cuda_identity
        && left.resolved_cuda_ordinal == right.resolved_cuda_ordinal
        && left.resolved_cuda_identity == right.resolved_cuda_identity
        && left.provider_implementation == right.provider_implementation
        && left.provider_binary_sha256 == right.provider_binary_sha256
        && left.runtime_version == right.runtime_version
        && left.model_ref == right.model_ref
        && left.model_sha256 == right.model_sha256
        && left.cpu_ep_registered == right.cpu_ep_registered
        && left.cpu_fallback_disabled == right.cpu_fallback_disabled
        && left.fallback_forbidden == right.fallback_forbidden
        && left.fallback_observed == right.fallback_observed
        && left.complete == right.complete
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainedTaskTrace {
    PackageAdmitted {
        task_label: String,
        package_label: String,
        package_sha256: String,
    },
    RunStarted,
    CaptureCompleted {
        width: u32,
        height: u32,
    },
    RecognitionCompleted {
        candidate_pages: Vec<String>,
        page_label: Option<String>,
        width: u32,
        height: u32,
    },
    RecognitionStarted {
        candidate_pages: Vec<String>,
        width: u32,
        height: u32,
    },
    StepStarted {
        step_index: u32,
        operation_label: String,
        from_page: String,
    },
    EffectIntent {
        step_index: u32,
        operation_label: String,
        action: InputAction,
        sampling: Option<InputSamplingEvidence>,
        guard: ContainedTaskGuardOutcome,
    },
    EffectCompleted {
        step_index: u32,
        operation_label: String,
    },
    StepFinished {
        step_index: u32,
        operation_label: String,
        page_label: String,
    },
    StabilityBaseline {
        step_index: u32,
        operation_label: String,
        declaration: StabilityTerminationDeclaration,
    },
    StabilityComparison {
        step_index: u32,
        operation_label: String,
        declaration: StabilityTerminationDeclaration,
        result: StabilityComparisonResult,
        prior_consecutive_unchanged: u32,
        new_consecutive_unchanged: u32,
        terminal_reason: Option<StabilityTerminalReason>,
    },
    StabilityTerminal {
        step_index: u32,
        operation_label: String,
        reason: StabilityTerminalReason,
    },
    PostAdmissionOcrObservation {
        frame_index: u32,
        observation: PostAdmissionOcrObservation,
    },
    PostAdmissionOcrComparison {
        report: PostAdmissionOcrComparisonReport,
    },
    Finalizing {
        outcome: TaskOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContainedTaskGuardOutcome {
    TrustedCoordinate,
    Passed {
        page_label: String,
        target_id: String,
        target_kind: String,
    },
}

/// Runtime boundary used by the semantic engine for device effects and durable facts.
pub trait ContainedTaskRuntime {
    type Error;

    fn capture(&mut self) -> Result<Frame, Self::Error>;

    fn action_seed(
        &mut self,
        _step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(None)
    }

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error>;

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedTaskOutcome {
    pub outcome: TaskOutcome,
    pub final_page: Option<String>,
    pub executed_steps: u32,
    pub selected_scheduling_outcome: Option<String>,
}

pub struct PreparedContainedTask {
    control: TaskControl,
    program: TaskProgram,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    scheduling_outcome: Option<SchedulingOutcomeDeclaration>,
    post_admission_ocr: Option<PreparedPostAdmissionOcr>,
    package_sha256: String,
    entry_count: usize,
    task_count: usize,
}

impl PreparedContainedTask {
    pub fn load(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
    ) -> Result<Self, ContainedTaskError> {
        Self::load_with_optional_vision_provider(instance_label, zip_bytes, expected, None)
    }

    pub fn load_with_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
        vision_provider: Arc<dyn VisionProvider>,
    ) -> Result<Self, ContainedTaskError> {
        Self::load_with_optional_vision_provider(
            instance_label,
            zip_bytes,
            expected,
            Some(vision_provider),
        )
    }

    fn load_with_optional_vision_provider(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
        vision_provider: Option<Arc<dyn VisionProvider>>,
    ) -> Result<Self, ContainedTaskError> {
        let bundle = match vision_provider {
            Some(provider) => ExternallyVerifiedBundle::load_with_vision_provider(
                instance_label,
                zip_bytes,
                expected,
                provider,
            ),
            None => ExternallyVerifiedBundle::load(instance_label, zip_bytes, expected),
        }
        .map_err(contained_task_admission_error)?;
        let package_sha256 = bundle.loaded_bundle().verified_hash().to_string();
        let entry_count = bundle.loaded_bundle().entry_count();
        let task_count = bundle.loaded_bundle().task_count();
        let bundle = bundle.into_loaded_bundle();
        let control = bundle
            .control()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_control_missing"))?;
        let control: TaskControl = serde_json::from_value(control)
            .map_err(|_| ContainedTaskError::new("contained_task_control_invalid"))?;
        control.validate()?;
        let program: TaskProgram = serde_json::from_value(bundle.operation().clone())
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let evaluator = bundle
            .evaluator()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_recognition_pack_missing"))?;
        let detector = bundle
            .detector()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_page_set_missing"))?;
        detector
            .validate(&evaluator)
            .map_err(|_| ContainedTaskError::new("contained_task_recognition_invalid"))?;
        program.validate(&control, &bundle, &detector)?;
        let post_admission_ocr =
            program.prepare_post_admission_ocr(&control, &bundle, &detector, &evaluator)?;
        let scheduling_outcome = program.scheduling_outcome.clone();
        Ok(Self {
            control,
            program,
            evaluator,
            detector,
            scheduling_outcome,
            post_admission_ocr,
            package_sha256,
            entry_count,
            task_count,
        })
    }

    pub fn task_label(&self) -> &str {
        &self.control.entry_task_id
    }

    pub fn package_label(&self) -> &str {
        &self.control.package_id
    }

    pub fn package_sha256(&self) -> &str {
        &self.package_sha256
    }

    pub fn execution_mode(&self) -> &str {
        &self.control.execution_mode
    }

    pub fn game(&self) -> &str {
        &self.control.game
    }

    pub fn scheduling_outcome(&self) -> Option<&SchedulingOutcomeDeclaration> {
        self.scheduling_outcome.as_ref()
    }

    pub fn stability_termination(&self) -> Option<&StabilityTerminationDeclaration> {
        self.control.stability_termination.as_ref()
    }

    pub const fn has_post_admission_ocr(&self) -> bool {
        self.post_admission_ocr.is_some()
    }

    pub const fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub const fn task_count(&self) -> usize {
        self.task_count
    }

    pub fn run<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::PackageAdmitted {
                task_label: self.task_label().to_string(),
                package_label: self.package_label().to_string(),
                package_sha256: self.package_sha256.clone(),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        runtime
            .record(ContainedTaskTrace::RunStarted)
            .map_err(ContainedTaskRunError::Boundary)?;

        let capture_interval = Duration::from_millis(
            self.control
                .capture_interval_ms
                .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS),
        );
        let step_timeout = Duration::from_millis(
            self.control
                .step_timeout_ms
                .unwrap_or(DEFAULT_STEP_TIMEOUT_MS),
        );
        let task_timeout =
            Duration::from_millis(self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS));
        let started = Instant::now();
        let mut ocr_collector = PostAdmissionOcrCollector::new(self.post_admission_ocr.as_ref());
        let mut observation =
            self.capture_until_page(runtime, &mut ocr_collector, step_timeout, capture_interval)?;
        if self.control.execution_mode == "recognize_only" {
            runtime
                .record(ContainedTaskTrace::Finalizing {
                    outcome: TaskOutcome::Success,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            return Ok(ContainedTaskOutcome {
                outcome: TaskOutcome::Success,
                final_page: Some(observation.page_label),
                executed_steps: 0,
                selected_scheduling_outcome: None,
            });
        }

        let candidates = self
            .program
            .operations
            .iter()
            .map(|operation| RunOperationCandidate::new(&operation.id, &operation.from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let config = RunStateConfig::new_with_target_pages(
            &self.control.game,
            self.program.target_pages()?,
            self.control.stop_on_confirmation.unwrap_or(true),
            1,
            self.control.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
        )
        .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let mut machine = RunStateMachine::new(config, 0)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        machine
            .observe_page(Some(observation.page_label.clone()))
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        let mut stability_tracker = StabilityTracker::default();

        loop {
            if started.elapsed() > task_timeout {
                return Err(ContainedTaskError::new("contained_task_timeout").into());
            }
            match machine
                .next_directive(&candidates)
                .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?
            {
                RunDirective::AwaitPage => {
                    observation = self.capture_until_page(
                        runtime,
                        &mut ocr_collector,
                        step_timeout,
                        capture_interval,
                    )?;
                    machine
                        .observe_page(Some(observation.page_label.clone()))
                        .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
                }
                RunDirective::ExecuteOperation {
                    operation_id,
                    current_page: from_page,
                    step_index,
                } => {
                    let operation = self
                        .program
                        .operations
                        .iter()
                        .find(|candidate| candidate.id == operation_id)
                        .ok_or_else(|| {
                            ContainedTaskError::new("contained_task_operation_missing")
                        })?;
                    let retry_policy = operation.retry_policy(
                        self.program.defaults,
                        self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
                    )?;
                    let mut attempt = 1;
                    loop {
                        if started.elapsed() > task_timeout {
                            return Err(ContainedTaskError::new("contained_task_timeout").into());
                        }
                        runtime
                            .record(ContainedTaskTrace::StepStarted {
                                step_index,
                                operation_label: operation_id.clone(),
                                from_page: from_page.clone(),
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        let (guard, target) = match operation.guard_outcome(
                            &self.control,
                            &observation,
                            &self.evaluator,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                let Some(policy) = retry_policy.as_ref() else {
                                    return Err(error.into());
                                };
                                match operation.failure_decision(
                                    policy,
                                    attempt,
                                    error.code(),
                                    Some(observation.page_label.clone()),
                                    RunFailureStage::PreExecutionGuard,
                                )? {
                                    RunOperationFailureDecision::RequestRecovery(trigger) => {
                                        machine.operation_needs_recovery(trigger).map_err(
                                            |_| {
                                                ContainedTaskError::new(
                                                    "contained_task_state_invalid",
                                                )
                                            },
                                        )?;
                                        break;
                                    }
                                    RunOperationFailureDecision::Fail(_) => {
                                        return Err(error.into());
                                    }
                                    RunOperationFailureDecision::Retry { .. } => {
                                        return Err(ContainedTaskError::new(
                                            "contained_task_state_invalid",
                                        )
                                        .into());
                                    }
                                }
                            }
                        };
                        let action_seed = runtime
                            .action_seed(step_index, &operation_id)
                            .map_err(ContainedTaskRunError::Boundary)?;
                        let (action, sampling) = operation.click.input_action(
                            &self.control.resolution,
                            target.as_ref(),
                            action_seed,
                        )?;
                        runtime
                            .record(ContainedTaskTrace::EffectIntent {
                                step_index,
                                operation_label: operation_id.clone(),
                                action: action.clone(),
                                sampling,
                                guard,
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        runtime
                            .input(action)
                            .map_err(ContainedTaskRunError::Boundary)?;
                        runtime
                            .record(ContainedTaskTrace::EffectCompleted {
                                step_index,
                                operation_label: operation_id.clone(),
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        Self::wait_post_input_delay(operation, started, task_timeout)?;
                        let destination_pages = operation.destination_pages()?;
                        if destination_pages.is_empty() {
                            observation = self.capture_until_page(
                                runtime,
                                &mut ocr_collector,
                                step_timeout,
                                capture_interval,
                            )?;
                            if let Some(reason) = self.complete_successful_step(
                                runtime,
                                &mut machine,
                                &mut stability_tracker,
                                step_index,
                                &operation_id,
                                &observation,
                            )? {
                                return self.finish_stability_termination(
                                    runtime,
                                    ocr_collector,
                                    &machine,
                                    &observation,
                                    reason,
                                );
                            }
                            break;
                        }
                        let confirmation_timeout = Duration::from_millis(
                            operation
                                .expect_after
                                .as_ref()
                                .and_then(|expectation| expectation.timeout_ms)
                                .unwrap_or(step_timeout.as_millis() as u64),
                        );
                        let confirmation_interval = Duration::from_millis(
                            operation
                                .expect_after
                                .as_ref()
                                .and_then(|expectation| expectation.interval_ms)
                                .unwrap_or(capture_interval.as_millis() as u64),
                        );
                        let (failed_observation, hit_error_page) = match self.await_postcondition(
                            runtime,
                            &mut ocr_collector,
                            operation,
                            confirmation_timeout,
                            confirmation_interval,
                        )? {
                            PostconditionResolution::Reached(reached) => {
                                observation = reached;
                                if let Some(reason) = self.complete_successful_step(
                                    runtime,
                                    &mut machine,
                                    &mut stability_tracker,
                                    step_index,
                                    &operation_id,
                                    &observation,
                                )? {
                                    return self.finish_stability_termination(
                                        runtime,
                                        ocr_collector,
                                        &machine,
                                        &observation,
                                        reason,
                                    );
                                }
                                break;
                            }
                            PostconditionResolution::Failed {
                                observation,
                                hit_error_page,
                            } => (observation, hit_error_page),
                        };
                        let after_page = failed_observation
                            .as_ref()
                            .map(|observation| observation.page_label.clone());
                        let Some(policy) = retry_policy.as_ref() else {
                            Self::finish_effect_attempt(
                                runtime,
                                step_index,
                                &operation_id,
                                failed_observation.as_ref(),
                            )?;
                            return Err(ContainedTaskError::with_detail(
                                "page_confirmation_failed",
                                format!(
                                    "operation={operation_id} attempts={attempt} after_page={} hit_error_page={hit_error_page}",
                                    after_page.as_deref().unwrap_or("<unrecognized>")
                                ),
                            )
                            .into());
                        };
                        match operation.failure_decision(
                            policy,
                            attempt,
                            "page_confirmation_failed",
                            after_page,
                            RunFailureStage::PostExecution { hit_error_page },
                        )? {
                            RunOperationFailureDecision::Retry {
                                next_attempt,
                                delay_ms,
                            } => {
                                let delay = Duration::from_millis(delay_ms);
                                if task_timeout
                                    .checked_sub(started.elapsed())
                                    .is_none_or(|remaining| delay > remaining)
                                {
                                    return Err(
                                        ContainedTaskError::new("contained_task_timeout").into()
                                    );
                                }
                                thread::sleep(delay);
                                match self.await_postcondition(
                                    runtime,
                                    &mut ocr_collector,
                                    operation,
                                    confirmation_timeout,
                                    confirmation_interval,
                                )? {
                                    PostconditionResolution::Reached(reached) => {
                                        observation = reached;
                                        if let Some(reason) = self.complete_successful_step(
                                            runtime,
                                            &mut machine,
                                            &mut stability_tracker,
                                            step_index,
                                            &operation_id,
                                            &observation,
                                        )? {
                                            return self.finish_stability_termination(
                                                runtime,
                                                ocr_collector,
                                                &machine,
                                                &observation,
                                                reason,
                                            );
                                        }
                                        break;
                                    }
                                    PostconditionResolution::Failed {
                                        observation: fresh,
                                        hit_error_page: true,
                                    } => {
                                        let after_page = fresh
                                            .as_ref()
                                            .map(|observation| observation.page_label.clone());
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            fresh.as_ref(),
                                        )?;
                                        match operation.failure_decision(
                                            policy,
                                            attempt,
                                            "page_confirmation_failed",
                                            after_page,
                                            RunFailureStage::PostExecution {
                                                hit_error_page: true,
                                            },
                                        )? {
                                            RunOperationFailureDecision::RequestRecovery(
                                                trigger,
                                            ) => {
                                                machine.operation_needs_recovery(trigger).map_err(
                                                    |_| {
                                                        ContainedTaskError::new(
                                                            "contained_task_state_invalid",
                                                        )
                                                    },
                                                )?;
                                                break;
                                            }
                                            RunOperationFailureDecision::Fail(_) => {
                                                return Err(ContainedTaskError::with_detail(
                                                    "contained_task_requires_scheduler",
                                                    format!(
                                                        "operation={operation_id} attempts={attempt} reason=page_confirmation_failed"
                                                    ),
                                                )
                                                .into());
                                            }
                                            RunOperationFailureDecision::Retry { .. } => {
                                                return Err(ContainedTaskError::new(
                                                    "contained_task_state_invalid",
                                                )
                                                .into());
                                            }
                                        }
                                    }
                                    PostconditionResolution::Failed {
                                        observation: Some(fresh),
                                        hit_error_page: false,
                                    } => {
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            Some(&fresh),
                                        )?;
                                        observation = fresh;
                                    }
                                    PostconditionResolution::Failed {
                                        observation: None,
                                        hit_error_page: false,
                                    } => {
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            None,
                                        )?;
                                        return Err(ContainedTaskError::with_detail(
                                            "page_confirmation_failed",
                                            format!(
                                                "operation={operation_id} attempts={attempt} after_page=<unrecognized> hit_error_page=false"
                                            ),
                                        )
                                        .into());
                                    }
                                }
                                attempt = next_attempt;
                            }
                            RunOperationFailureDecision::RequestRecovery(trigger) => {
                                Self::finish_effect_attempt(
                                    runtime,
                                    step_index,
                                    &operation_id,
                                    failed_observation.as_ref(),
                                )?;
                                machine.operation_needs_recovery(trigger).map_err(|_| {
                                    ContainedTaskError::new("contained_task_state_invalid")
                                })?;
                                break;
                            }
                            RunOperationFailureDecision::Fail(_) => {
                                Self::finish_effect_attempt(
                                    runtime,
                                    step_index,
                                    &operation_id,
                                    failed_observation.as_ref(),
                                )?;
                                return Err(ContainedTaskError::with_detail(
                                    "contained_task_requires_scheduler",
                                    format!(
                                        "operation={operation_id} attempts={attempt} reason=page_confirmation_failed"
                                    ),
                                )
                                .into());
                            }
                        }
                    }
                }
                RunDirective::Continue { .. } => {
                    return Err(ContainedTaskError::new("contained_task_state_invalid").into());
                }
                RunDirective::Terminal(RunTerminal::Completed { current_page }) => {
                    return Self::finish_success(
                        runtime,
                        ocr_collector,
                        current_page,
                        machine.completed_steps(),
                    );
                }
                RunDirective::Terminal(
                    RunTerminal::SuccessorSuggested { .. } | RunTerminal::PausedNeedsHuman { .. },
                ) => {
                    return Err(ContainedTaskError::new("contained_task_requires_scheduler").into());
                }
            }
        }
    }

    fn complete_successful_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        machine: &mut RunStateMachine,
        stability_tracker: &mut StabilityTracker,
        step_index: u32,
        operation_label: &str,
        observation: &PageObservation,
    ) -> Result<Option<StabilityTerminalReason>, ContainedTaskRunError<R::Error>> {
        let terminal_reason = if let Some(declaration) = &self.control.stability_termination {
            let sample = observation.stability_sample.clone().ok_or_else(|| {
                ContainedTaskError::new("contained_task_stability_capture_invalid")
            })?;
            let transition = stability_tracker.propose(sample, declaration, step_index)?;
            let (trace, terminal_reason) = match &transition {
                StabilityTransition::Baseline { .. } => (
                    ContainedTaskTrace::StabilityBaseline {
                        step_index,
                        operation_label: operation_label.to_string(),
                        declaration: declaration.clone(),
                    },
                    None,
                ),
                StabilityTransition::Comparison {
                    result,
                    prior_consecutive_unchanged,
                    new_consecutive_unchanged,
                    terminal_reason,
                    ..
                } => (
                    ContainedTaskTrace::StabilityComparison {
                        step_index,
                        operation_label: operation_label.to_string(),
                        declaration: declaration.clone(),
                        result: *result,
                        prior_consecutive_unchanged: *prior_consecutive_unchanged,
                        new_consecutive_unchanged: *new_consecutive_unchanged,
                        terminal_reason: *terminal_reason,
                    },
                    *terminal_reason,
                ),
            };
            runtime
                .record(trace)
                .map_err(ContainedTaskRunError::Boundary)?;
            stability_tracker.commit(transition);
            terminal_reason
        } else {
            None
        };

        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation_label.to_string(),
                page_label: observation.page_label.clone(),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        machine
            .operation_succeeded(operation_label, Some(observation.page_label.clone()))
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        if let Some(reason) = terminal_reason {
            runtime
                .record(ContainedTaskTrace::StabilityTerminal {
                    step_index,
                    operation_label: operation_label.to_string(),
                    reason,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
        }
        Ok(terminal_reason)
    }

    fn finish_stability_termination<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: PostAdmissionOcrCollector<'_>,
        machine: &RunStateMachine,
        observation: &PageObservation,
        reason: StabilityTerminalReason,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        match reason {
            StabilityTerminalReason::ConsecutiveUnchangedThresholdReached => Self::finish_success(
                runtime,
                ocr_collector,
                Some(observation.page_label.clone()),
                machine.completed_steps(),
            ),
            StabilityTerminalReason::MaxStepsReached => {
                Err(ContainedTaskError::new("contained_task_requires_scheduler").into())
            }
        }
    }

    fn finish_success<R: ContainedTaskRuntime>(
        runtime: &mut R,
        ocr_collector: PostAdmissionOcrCollector<'_>,
        final_page: Option<String>,
        executed_steps: u32,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        let selected_scheduling_outcome = match ocr_collector.finish()? {
            Some(report) => {
                let outcome_key = report.outcome_key.clone();
                runtime
                    .record(ContainedTaskTrace::PostAdmissionOcrComparison { report })
                    .map_err(ContainedTaskRunError::Boundary)?;
                Some(outcome_key)
            }
            None => None,
        };
        runtime
            .record(ContainedTaskTrace::Finalizing {
                outcome: TaskOutcome::Success,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        Ok(ContainedTaskOutcome {
            outcome: TaskOutcome::Success,
            final_page,
            executed_steps,
            selected_scheduling_outcome,
        })
    }

    fn finish_effect_attempt<R: ContainedTaskRuntime>(
        runtime: &mut R,
        step_index: u32,
        operation_label: &str,
        observation: Option<&PageObservation>,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation_label.to_string(),
                page_label: match observation {
                    Some(observation) => observation.page_label.clone(),
                    None => "<unrecognized>".to_string(),
                },
            })
            .map_err(ContainedTaskRunError::Boundary)
    }

    fn wait_post_input_delay(
        operation: &TaskOperation,
        started: Instant,
        task_timeout: Duration,
    ) -> Result<(), ContainedTaskError> {
        let Some(delay_ms) = operation.post_delay_ms else {
            return Ok(());
        };
        let delay = Duration::from_millis(delay_ms);
        if task_timeout
            .checked_sub(started.elapsed())
            .is_none_or(|remaining| delay >= remaining)
        {
            return Err(ContainedTaskError::new("contained_task_timeout"));
        }
        thread::sleep(delay);
        if started.elapsed() >= task_timeout {
            Err(ContainedTaskError::new("contained_task_timeout"))
        } else {
            Ok(())
        }
    }

    fn capture_until_page<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        timeout: Duration,
        interval: Duration,
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        loop {
            if let Some(observation) = self.capture_page(runtime, ocr_collector)? {
                return Ok(observation);
            }
            if started.elapsed() >= timeout {
                return Err(ContainedTaskError::new("contained_task_page_unknown").into());
            }
            thread::sleep(interval);
        }
    }

    fn capture_page<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let frame = runtime.capture().map_err(ContainedTaskRunError::Boundary)?;
        self.control.resolution.validate_frame(&frame)?;
        let stability_sample = self
            .control
            .stability_termination
            .as_ref()
            .map(|declaration| stability_sample(&frame, declaration))
            .transpose()?;
        runtime
            .record(ContainedTaskTrace::CaptureCompleted {
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let scene = scene_from_frame(&frame)?;
        let candidate_pages = self
            .detector
            .page_ids()
            .map(str::to_string)
            .collect::<Vec<_>>();
        runtime
            .record(ContainedTaskTrace::RecognitionStarted {
                candidate_pages: candidate_pages.clone(),
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let matched_pages = self
            .detector
            .evaluate_all(&self.evaluator, &scene)
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    "contained_task_recognition_failed",
                    error.to_string(),
                )
            })?
            .into_iter()
            .filter(|evaluation| evaluation.matched)
            .map(|evaluation| evaluation.page_id)
            .collect::<Vec<_>>();
        if matched_pages.len() > 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_recognition_conflict",
                matched_pages.join(","),
            )
            .into());
        }
        let page = matched_pages.into_iter().next();
        runtime
            .record(ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label: page.clone(),
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let Some(page_label) = page else {
            return Ok(None);
        };
        if let Some((frame_index, observation)) =
            ocr_collector.observe(&self.control.game, &self.evaluator, &page_label, &scene)?
        {
            runtime
                .record(ContainedTaskTrace::PostAdmissionOcrObservation {
                    frame_index,
                    observation,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
        }
        Ok(Some(PageObservation {
            page_label,
            scene,
            stability_sample,
        }))
    }

    fn await_postcondition<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        operation: &TaskOperation,
        timeout: Duration,
        interval: Duration,
    ) -> Result<PostconditionResolution, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        let mut last_observation = None;
        loop {
            if let Some(observation) = self.capture_page(runtime, ocr_collector)? {
                let destination_matches =
                    operation.matching_destination_count(&self.control, &observation)?;
                let hit_error_page = self
                    .program
                    .is_error_page(&self.control, &observation.page_label);
                if destination_matches > 1 || (destination_matches == 1 && hit_error_page) {
                    return Err(ContainedTaskError::with_detail(
                        "contained_task_recognition_conflict",
                        observation.page_label,
                    )
                    .into());
                }
                if destination_matches == 1 {
                    return Ok(PostconditionResolution::Reached(observation));
                }
                if hit_error_page {
                    return Ok(PostconditionResolution::Failed {
                        observation: Some(observation),
                        hit_error_page: true,
                    });
                }
                last_observation = Some(observation);
            }
            if started.elapsed() >= timeout {
                return Ok(PostconditionResolution::Failed {
                    observation: last_observation,
                    hit_error_page: false,
                });
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            thread::sleep(interval.min(remaining));
        }
    }
}

struct PageObservation {
    page_label: String,
    scene: Scene,
    stability_sample: Option<StabilityFrameSample>,
}

enum PostconditionResolution {
    Reached(PageObservation),
    Failed {
        observation: Option<PageObservation>,
        hit_error_page: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StabilityFrameSample {
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    pixels: Vec<u8>,
}

fn stability_sample(
    frame: &Frame,
    declaration: &StabilityTerminationDeclaration,
) -> Result<StabilityFrameSample, ContainedTaskError> {
    let bytes_per_pixel = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let frame_width = usize::try_from(frame.width)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let frame_height = usize::try_from(frame.height)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let expected_frame_len = frame_width
        .checked_mul(frame_height)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    if frame.pixels.len() != expected_frame_len {
        return Err(ContainedTaskError::new(
            "contained_task_stability_capture_invalid",
        ));
    }

    let region = declaration.region;
    let end_x = region
        .x
        .checked_add(region.width)
        .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let end_y = region
        .y
        .checked_add(region.height)
        .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    if region.width == 0 || region.height == 0 || end_x > frame.width || end_y > frame.height {
        return Err(ContainedTaskError::new(
            "contained_task_stability_capture_invalid",
        ));
    }

    let x = usize::try_from(region.x)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let y = usize::try_from(region.y)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let width = usize::try_from(region.width)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let height = usize::try_from(region.height)
        .map_err(|_| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let capacity = row_bytes
        .checked_mul(height)
        .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
    let mut pixels = Vec::with_capacity(capacity);
    for row in y..y + height {
        let start = row
            .checked_mul(frame_width)
            .and_then(|offset| offset.checked_add(x))
            .and_then(|offset| offset.checked_mul(bytes_per_pixel))
            .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
        let end = start
            .checked_add(row_bytes)
            .ok_or_else(|| ContainedTaskError::new("contained_task_stability_capture_invalid"))?;
        pixels.extend_from_slice(
            frame.pixels.get(start..end).ok_or_else(|| {
                ContainedTaskError::new("contained_task_stability_capture_invalid")
            })?,
        );
    }

    Ok(StabilityFrameSample {
        width: region.width,
        height: region.height,
        pixel_format: frame.pixel_format,
        pixels,
    })
}

fn compare_stability_samples(
    previous: &StabilityFrameSample,
    current: &StabilityFrameSample,
    comparison: &StabilityComparisonDeclaration,
) -> Result<StabilityComparisonResult, ContainedTaskError> {
    if previous.width != current.width
        || previous.height != current.height
        || previous.pixel_format != current.pixel_format
    {
        return Err(ContainedTaskError::new(
            "contained_task_stability_comparison_failed",
        ));
    }
    match comparison.mode {
        StabilityComparisonMode::ExactPixelsV1 => {
            if previous.pixels == current.pixels {
                Ok(StabilityComparisonResult::Unchanged)
            } else {
                Ok(StabilityComparisonResult::Changed)
            }
        }
    }
}

#[derive(Debug, Default)]
struct StabilityTracker {
    previous: Option<StabilityFrameSample>,
    consecutive_unchanged: u32,
}

enum StabilityTransition {
    Baseline {
        sample: StabilityFrameSample,
    },
    Comparison {
        sample: StabilityFrameSample,
        result: StabilityComparisonResult,
        prior_consecutive_unchanged: u32,
        new_consecutive_unchanged: u32,
        terminal_reason: Option<StabilityTerminalReason>,
    },
}

impl StabilityTracker {
    fn propose(
        &self,
        sample: StabilityFrameSample,
        declaration: &StabilityTerminationDeclaration,
        step_index: u32,
    ) -> Result<StabilityTransition, ContainedTaskError> {
        let Some(previous) = self.previous.as_ref() else {
            return Ok(StabilityTransition::Baseline { sample });
        };
        let result = compare_stability_samples(previous, &sample, &declaration.comparison)?;
        let prior_consecutive_unchanged = self.consecutive_unchanged;
        let new_consecutive_unchanged = match result {
            StabilityComparisonResult::Changed => 0,
            StabilityComparisonResult::Unchanged => {
                prior_consecutive_unchanged.checked_add(1).ok_or_else(|| {
                    ContainedTaskError::new("contained_task_stability_counter_overflow")
                })?
            }
        };
        if new_consecutive_unchanged > declaration.consecutive_unchanged_threshold {
            return Err(ContainedTaskError::new(
                "contained_task_stability_counter_invalid",
            ));
        }
        let completed_steps = step_index
            .checked_add(1)
            .ok_or_else(|| ContainedTaskError::new("contained_task_stability_counter_overflow"))?;
        if completed_steps > declaration.max_steps {
            return Err(ContainedTaskError::new(
                "contained_task_stability_counter_invalid",
            ));
        }
        let terminal_reason =
            if new_consecutive_unchanged == declaration.consecutive_unchanged_threshold {
                Some(StabilityTerminalReason::ConsecutiveUnchangedThresholdReached)
            } else if completed_steps == declaration.max_steps {
                Some(StabilityTerminalReason::MaxStepsReached)
            } else {
                None
            };
        Ok(StabilityTransition::Comparison {
            sample,
            result,
            prior_consecutive_unchanged,
            new_consecutive_unchanged,
            terminal_reason,
        })
    }

    fn commit(&mut self, transition: StabilityTransition) {
        match transition {
            StabilityTransition::Baseline { sample } => {
                self.previous = Some(sample);
                self.consecutive_unchanged = 0;
            }
            StabilityTransition::Comparison {
                sample,
                new_consecutive_unchanged,
                ..
            } => {
                self.previous = Some(sample);
                self.consecutive_unchanged = new_consecutive_unchanged;
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TaskControl {
    schema_version: String,
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    #[serde(default)]
    capture_interval_ms: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    step_timeout_ms: Option<u64>,
    #[serde(default)]
    max_steps: Option<u32>,
    #[serde(default)]
    stop_on_confirmation: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    stability_termination: Option<StabilityTerminationDeclaration>,
}

impl TaskControl {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.schema_version != CONTROL_SCHEMA
            || self.package_id.trim().is_empty()
            || self.game.trim().is_empty()
            || self.server.trim().is_empty()
            || self.entry_task_id.trim().is_empty()
            || !matches!(
                self.execution_mode.as_str(),
                "recognize_only" | "navigable_route" | "in_page_guard"
            )
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        self.resolution.validate()?;
        validate_bounded(self.capture_interval_ms, MAX_CAPTURE_INTERVAL_MS)?;
        validate_bounded(self.timeout_ms, MAX_TASK_TIMEOUT_MS)?;
        validate_bounded(self.step_timeout_ms, MAX_STEP_TIMEOUT_MS)?;
        if self
            .max_steps
            .is_some_and(|value| value == 0 || value > MAX_STEPS)
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        match (
            self.execution_mode.as_str(),
            self.stability_termination.as_ref(),
        ) {
            ("in_page_guard", Some(declaration)) => {
                declaration.validate(&self.resolution, self.max_steps)?;
            }
            (_, Some(_)) => {
                return Err(ContainedTaskError::new("contained_task_control_invalid"));
            }
            (_, None) => {}
        }
        Ok(())
    }
}

fn validate_bounded(value: Option<u64>, maximum: u64) -> Result<(), ContainedTaskError> {
    if value.is_some_and(|value| value == 0 || value > maximum) {
        Err(ContainedTaskError::new("contained_task_control_invalid"))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

impl Resolution {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.width == 0 || self.height == 0 {
            Err(ContainedTaskError::new("contained_task_resolution_invalid"))
        } else {
            Ok(())
        }
    }

    fn validate_frame(&self, frame: &Frame) -> Result<(), ContainedTaskError> {
        if frame.width == self.width && frame.height == self.height {
            Ok(())
        } else {
            Err(ContainedTaskError::new(
                "contained_task_frame_resolution_mismatch",
            ))
        }
    }

    fn validate_point(&self, x: i32, y: i32) -> Result<(), ContainedTaskError> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            Err(ContainedTaskError::new(
                "contained_task_input_out_of_bounds",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize)]
struct TaskProgram {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    coordinate_space: Resolution,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    timeout_ms: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    max_steps: Option<u32>,
    #[serde(default)]
    target_page: Option<PageDeclaration>,
    #[serde(default)]
    error_pages: Vec<String>,
    #[serde(default)]
    scheduling_outcome: Option<SchedulingOutcomeDeclaration>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    post_admission_ocr: Option<PostAdmissionOcrDeclaration>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    stability_termination: Option<StabilityTerminationDeclaration>,
    #[serde(default)]
    recovery: Option<TaskRecovery>,
    #[serde(default)]
    defaults: TaskOperationDefaults,
    operations: Vec<TaskOperation>,
}

impl TaskProgram {
    fn validate(
        &self,
        control: &TaskControl,
        bundle: &LoadedBundle,
        detector: &PageDetector,
    ) -> Result<(), ContainedTaskError> {
        let schema_valid = match self.schema_version.as_str() {
            "0.3" | "0.4" | "0.5" | "0.6" => self.post_admission_ocr.is_none(),
            "0.7" => self.post_admission_ocr.is_some(),
            _ => false,
        };
        if !schema_valid
            || self.task_id != control.entry_task_id
            || self.game != control.game
            || (!self.server_scope.is_empty()
                && !self
                    .server_scope
                    .iter()
                    .any(|value| value == &control.server))
            || self.coordinate_space.width != control.resolution.width
            || self.coordinate_space.height != control.resolution.height
            || self.operations.is_empty()
            || self.error_pages.iter().any(|value| value.trim().is_empty())
        {
            return Err(ContainedTaskError::new("contained_task_program_invalid"));
        }
        self.validate_task_timeout(control)?;
        self.validate_task_max_steps(control)?;
        validate_stability_contract(control, self)?;
        let target_pages = self.target_pages()?;
        validate_page_references(&control.game, &target_pages, detector)?;
        validate_page_references(&control.game, &self.error_pages, detector)?;
        validate_page_set_overlap(&control.game, &target_pages, &self.error_pages, detector)?;
        if let Some(declaration) = &self.scheduling_outcome {
            validate_scheduling_outcome_execution_mode(control)?;
            declaration.validate().map_err(|_| {
                ContainedTaskError::new("contained_task_outcome_declaration_invalid")
            })?;
            if declaration
                .designated_operation()
                .is_some_and(|designated| {
                    self.operations
                        .iter()
                        .filter(|operation| operation.id == designated)
                        .count()
                        != 1
                })
            {
                return Err(ContainedTaskError::new(
                    "contained_task_outcome_declaration_invalid",
                ));
            }
            let terminal_pages = declaration
                .mappings()
                .iter()
                .flat_map(|mapping| mapping.terminal_pages().iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            validate_page_references(&control.game, &terminal_pages, detector)?;
            validate_page_set_overlap(&control.game, &terminal_pages, &self.error_pages, detector)?;
        }
        let mut operation_ids = BTreeSet::new();
        for operation in &self.operations {
            operation.validate(control, self.defaults)?;
            let destination_pages = operation.destination_pages()?;
            validate_page_references(&control.game, &destination_pages, detector)?;
            validate_page_set_overlap(
                &control.game,
                &destination_pages,
                &self.error_pages,
                detector,
            )?;
            if !operation_ids.insert(&operation.id) {
                return Err(ContainedTaskError::new("contained_task_program_invalid"));
            }
        }
        if let Some(declaration) = &self.scheduling_outcome {
            let observable_pages = detector.page_ids().map(str::to_owned).collect::<Vec<_>>();
            validate_scheduling_outcome_coverage(
                &control.game,
                &target_pages,
                &observable_pages,
                &self.operations,
                declaration,
            )?;
        }
        self.validate_recovery(bundle)?;
        Ok(())
    }

    fn validate_task_timeout(&self, control: &TaskControl) -> Result<(), ContainedTaskError> {
        let valid = match self.schema_version.as_str() {
            "0.3" | "0.4" | "0.5" | "0.6" => self.timeout_ms.is_none(),
            "0.7" => self
                .timeout_ms
                .is_none_or(|timeout_ms| control.timeout_ms == Some(timeout_ms)),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ContainedTaskError::new("contained_task_program_invalid"))
        }
    }

    fn validate_task_max_steps(&self, control: &TaskControl) -> Result<(), ContainedTaskError> {
        let valid = match self.schema_version.as_str() {
            "0.3" | "0.4" | "0.5" | "0.6" => self.max_steps.is_none(),
            "0.7" => self.max_steps.is_none_or(|max_steps| {
                (1..=MAX_STEPS).contains(&max_steps) && control.max_steps == Some(max_steps)
            }),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ContainedTaskError::new("contained_task_program_invalid"))
        }
    }

    fn prepare_post_admission_ocr(
        &self,
        control: &TaskControl,
        bundle: &LoadedBundle,
        detector: &PageDetector,
        evaluator: &RecognitionEvaluator,
    ) -> Result<Option<PreparedPostAdmissionOcr>, ContainedTaskError> {
        let Some(declaration) = &self.post_admission_ocr else {
            return Ok(None);
        };
        declaration.validate()?;
        let page_ids = declaration
            .page_ids()?
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let target_ids = declaration
            .target_ids()?
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        validate_page_references(&control.game, &page_ids, detector)?;
        validate_post_admission_ocr_page_gate(control, bundle, evaluator, &page_ids, &target_ids)?;
        for target_id in &target_ids {
            validate_post_admission_ocr_target(control, evaluator, target_id)?;
        }
        let scheduling = self.scheduling_outcome.as_ref().ok_or_else(|| {
            ContainedTaskError::new("contained_task_post_admission_ocr_outcome_missing")
        })?;
        if scheduling
            .mappings()
            .iter()
            .filter(|mapping| mapping.outcome_key() == declaration.outcome_key)
            .count()
            != 1
        {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_outcome_invalid",
            ));
        }
        let expected_hash = Sha256Hash::parse_hex(&declaration.truth_set.sha256).map_err(|_| {
            ContainedTaskError::new("contained_task_post_admission_ocr_truth_invalid")
        })?;
        if expected_hash.to_string() != declaration.truth_set.sha256 {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_truth_invalid",
            ));
        }
        let relative_path = format!("operations/{}/{}", self.task_id, declaration.truth_set.path);
        let bytes = bundle.resource_entry(&relative_path).map_err(|_| {
            ContainedTaskError::with_detail(
                "contained_task_post_admission_ocr_truth_missing",
                relative_path.clone(),
            )
        })?;
        if manifest_entry_sha256(bundle, &relative_path)? != expected_hash
            || u64::try_from(bytes.len())
                .map_or(true, |size| size > declaration.limits.max_total_bytes)
        {
            return Err(ContainedTaskError::with_detail(
                "contained_task_post_admission_ocr_truth_mismatch",
                relative_path,
            ));
        }
        let truth: PostAdmissionOcrTruthSet = serde_json::from_slice(bytes).map_err(|_| {
            ContainedTaskError::new("contained_task_post_admission_ocr_truth_invalid")
        })?;
        if truth.schema_version != POST_ADMISSION_OCR_TRUTH_SCHEMA
            || truth.items.is_empty()
            || u32::try_from(truth.items.len())
                .map_or(true, |count| count > declaration.limits.max_truth_entries)
        {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_truth_invalid",
            ));
        }
        let max_string_bytes = usize::try_from(declaration.limits.max_string_bytes)
            .map_err(|_| ContainedTaskError::new("contained_task_post_admission_ocr_invalid"))?;
        let mut normalized = BTreeSet::new();
        for item in truth.items {
            if item.len() > max_string_bytes {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_truth_invalid",
                ));
            }
            let item = normalize_post_admission_ocr(&item, declaration.normalization);
            if item.is_empty() || item.len() > max_string_bytes || !normalized.insert(item) {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_truth_invalid",
                ));
            }
        }
        Ok(Some(PreparedPostAdmissionOcr {
            declaration: declaration.clone(),
            page_ids,
            target_ids,
            truth: normalized.into_iter().collect(),
        }))
    }

    fn target_pages(&self) -> Result<Vec<String>, ContainedTaskError> {
        self.target_page
            .as_ref()
            .map(PageDeclaration::normalized)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn is_error_page(&self, control: &TaskControl, page_label: &str) -> bool {
        self.error_pages
            .iter()
            .any(|expected| crate::page_anchor_matches(&control.game, page_label, expected))
    }

    fn validate_recovery(&self, bundle: &LoadedBundle) -> Result<(), ContainedTaskError> {
        let mut recovery_tasks = BTreeSet::new();
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
            recovery_tasks.insert(recovery.task_id());
        }
        if self
            .operations
            .iter()
            .any(|operation| operation.on_error.is_some())
        {
            recovery_tasks.insert("return_home");
        }
        for task_id in recovery_tasks {
            let relative_path = format!("operations/{task_id}/task.json");
            let bytes = bundle.resource_entry(&relative_path).map_err(|_| {
                ContainedTaskError::with_detail(
                    "contained_task_recovery_missing",
                    relative_path.clone(),
                )
            })?;
            let recovery: TaskProgram = serde_json::from_slice(bytes).map_err(|_| {
                ContainedTaskError::with_detail(
                    "contained_task_recovery_invalid",
                    relative_path.clone(),
                )
            })?;
            if recovery.task_id != task_id || recovery.game != self.game {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_recovery_invalid",
                    relative_path,
                ));
            }
        }
        Ok(())
    }
}

fn validate_post_admission_ocr_page_gate(
    control: &TaskControl,
    bundle: &LoadedBundle,
    evaluator: &RecognitionEvaluator,
    page_ids: &[String],
    target_ids: &[String],
) -> Result<(), ContainedTaskError> {
    let pages_path = bundle.pages_path().ok_or_else(|| {
        ContainedTaskError::new("contained_task_post_admission_ocr_page_gate_missing")
    })?;
    let pages: PageSet = serde_json::from_slice(bundle.entry(pages_path).ok_or_else(|| {
        ContainedTaskError::new("contained_task_post_admission_ocr_page_gate_missing")
    })?)
    .map_err(|_| ContainedTaskError::new("contained_task_post_admission_ocr_page_gate_invalid"))?;
    validate_post_admission_ocr_page_set(control, evaluator, &pages, page_ids, target_ids)
}

fn validate_post_admission_ocr_page_set(
    control: &TaskControl,
    evaluator: &RecognitionEvaluator,
    pages: &PageSet,
    page_ids: &[String],
    target_ids: &[String],
) -> Result<(), ContainedTaskError> {
    if pages.pages.iter().any(|page| {
        page.required
            .iter()
            .chain(page.any_of.iter().flatten())
            .chain(page.optional.iter())
            .chain(page.forbidden.iter())
            .any(|target| target_ids.iter().any(|target_id| target == target_id))
    }) {
        return Err(ContainedTaskError::new(
            "contained_task_post_admission_ocr_target_in_page_gate",
        ));
    }
    for page_id in page_ids {
        let matching = pages
            .pages
            .iter()
            .filter(|page| crate::page_anchor_matches(&control.game, &page.id, page_id))
            .collect::<Vec<_>>();
        let [page] = matching.as_slice() else {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_page_gate_invalid",
            ));
        };
        let gate_targets = page
            .required
            .iter()
            .chain(page.any_of.iter().flatten())
            .chain(page.optional.iter())
            .chain(page.forbidden.iter())
            .collect::<Vec<_>>();
        let positive_count = page.required.len() + page.any_of.iter().map(Vec::len).sum::<usize>();
        if positive_count == 0
            || gate_targets.iter().any(|target| {
                !matches!(
                    evaluator.target_kind(target),
                    Ok(TargetKind::Template | TargetKind::Color)
                )
            })
        {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_page_gate_invalid",
            ));
        }
    }
    Ok(())
}

fn recognition_target_id(target: &RecognitionTarget) -> &str {
    match target {
        RecognitionTarget::Template(target) => &target.id,
        RecognitionTarget::Color(target) => &target.id,
        RecognitionTarget::ClickOnly(target) => &target.id,
        RecognitionTarget::Ocr(target) => &target.id,
        RecognitionTarget::Nn(target) => &target.id,
    }
}

fn validate_post_admission_ocr_target(
    control: &TaskControl,
    evaluator: &RecognitionEvaluator,
    target_id: &str,
) -> Result<(), ContainedTaskError> {
    let matching = evaluator
        .pack()
        .targets
        .iter()
        .filter(|target| recognition_target_id(target) == target_id)
        .collect::<Vec<_>>();
    let [RecognitionTarget::Ocr(target)] = matching.as_slice() else {
        return Err(ContainedTaskError::new(
            "contained_task_post_admission_ocr_target_invalid",
        ));
    };
    match &target.region {
        PackRegion::Keyword(value) if value == "full_frame" => Ok(()),
        PackRegion::Rect(rect) => {
            let x = i64::from(rect.x);
            let y = i64::from(rect.y);
            let width = i64::from(rect.width);
            let height = i64::from(rect.height);
            if x < 0
                || y < 0
                || width <= 0
                || height <= 0
                || x.checked_add(width)
                    .is_none_or(|end| end > i64::from(control.resolution.width))
                || y.checked_add(height)
                    .is_none_or(|end| end > i64::from(control.resolution.height))
            {
                return Err(ContainedTaskError::new(
                    "contained_task_post_admission_ocr_target_out_of_bounds",
                ));
            }
            Ok(())
        }
        PackRegion::Keyword(_) => Err(ContainedTaskError::new(
            "contained_task_post_admission_ocr_target_invalid",
        )),
    }
}

impl PostAdmissionOcrDeclaration {
    fn page_ids(&self) -> Result<Vec<&str>, ContainedTaskError> {
        match (&self.page_id, &self.page_ids) {
            (Some(page_id), None) if !page_id.trim().is_empty() => Ok(vec![page_id.as_str()]),
            (None, Some(page_ids)) if page_ids.len() == 2 => {
                let mut unique = BTreeSet::new();
                let mut ordered = Vec::with_capacity(page_ids.len());
                for page_id in page_ids {
                    if page_id.trim().is_empty() || !unique.insert(page_id.as_str()) {
                        return Err(ContainedTaskError::new(
                            "contained_task_post_admission_ocr_invalid",
                        ));
                    }
                    ordered.push(page_id.as_str());
                }
                Ok(ordered)
            }
            _ => Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_invalid",
            )),
        }
    }

    fn target_ids(&self) -> Result<Vec<&str>, ContainedTaskError> {
        match (&self.target_id, &self.target_ids) {
            (Some(target_id), None) if !target_id.trim().is_empty() => Ok(vec![target_id.as_str()]),
            (None, Some(target_ids))
                if !target_ids.is_empty() && target_ids.len() <= MAX_POST_ADMISSION_OCR_TARGETS =>
            {
                let mut unique = BTreeSet::new();
                let mut ordered = Vec::with_capacity(target_ids.len());
                for target_id in target_ids {
                    if target_id.trim().is_empty() || !unique.insert(target_id.as_str()) {
                        return Err(ContainedTaskError::new(
                            "contained_task_post_admission_ocr_invalid",
                        ));
                    }
                    ordered.push(target_id.as_str());
                }
                Ok(ordered)
            }
            _ => Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_invalid",
            )),
        }
    }

    fn validate(&self) -> Result<(), ContainedTaskError> {
        let limits = &self.limits;
        self.page_ids()?;
        self.target_ids()?;
        if self.outcome_key.trim().is_empty()
            || !safe_task_local_path(&self.truth_set.path)
            || limits.max_frames == 0
            || limits.max_frames > MAX_POST_ADMISSION_OCR_FRAMES
            || limits.max_items == 0
            || limits.max_items > MAX_POST_ADMISSION_OCR_ITEMS
            || limits.max_string_bytes == 0
            || limits.max_string_bytes > MAX_POST_ADMISSION_OCR_STRING_BYTES
            || limits.max_total_bytes == 0
            || limits.max_total_bytes > MAX_POST_ADMISSION_OCR_TOTAL_BYTES
            || limits.max_truth_entries == 0
            || limits.max_truth_entries > MAX_POST_ADMISSION_OCR_TRUTH_ENTRIES
        {
            return Err(ContainedTaskError::new(
                "contained_task_post_admission_ocr_invalid",
            ));
        }
        Ok(())
    }
}

fn safe_task_local_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn manifest_entry_sha256(
    bundle: &LoadedBundle,
    relative_path: &str,
) -> Result<Sha256Hash, ContainedTaskError> {
    let files = bundle
        .manifest()
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ContainedTaskError::new("contained_task_post_admission_ocr_truth_invalid")
        })?;
    let matching = files
        .iter()
        .filter(|entry| {
            entry.get("path").and_then(serde_json::Value::as_str) == Some(relative_path)
        })
        .collect::<Vec<_>>();
    let [entry] = matching.as_slice() else {
        return Err(ContainedTaskError::new(
            "contained_task_post_admission_ocr_truth_invalid",
        ));
    };
    entry
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ContainedTaskError::new("contained_task_post_admission_ocr_truth_invalid"))
        .and_then(|value| {
            Sha256Hash::parse_hex(value).map_err(|_| {
                ContainedTaskError::new("contained_task_post_admission_ocr_truth_invalid")
            })
        })
}

fn validate_stability_contract(
    control: &TaskControl,
    program: &TaskProgram,
) -> Result<(), ContainedTaskError> {
    match (
        control.stability_termination.as_ref(),
        program.stability_termination.as_ref(),
    ) {
        (Some(control_declaration), Some(program_declaration))
            if control.execution_mode == "in_page_guard"
                && control_declaration == program_declaration
                && program.target_page.is_none()
                && match (
                    program.scheduling_outcome.as_ref(),
                    program.post_admission_ocr.as_ref(),
                ) {
                    (None, None) => true,
                    (Some(scheduling), Some(post_admission_ocr)) => {
                        scheduling
                            .mappings()
                            .iter()
                            .filter(|mapping| {
                                mapping.outcome_key() == post_admission_ocr.outcome_key
                            })
                            .count()
                            == 1
                    }
                    _ => false,
                } =>
        {
            Ok(())
        }
        (None, None) => Ok(()),
        _ => Err(ContainedTaskError::new("contained_task_program_invalid")),
    }
}

fn validate_scheduling_outcome_execution_mode(
    control: &TaskControl,
) -> Result<(), ContainedTaskError> {
    if control.execution_mode == "recognize_only" {
        Err(ContainedTaskError::new(
            "contained_task_outcome_declaration_invalid",
        ))
    } else {
        Ok(())
    }
}

fn validate_scheduling_outcome_coverage(
    game: &str,
    target_pages: &[String],
    observable_pages: &[String],
    operations: &[TaskOperation],
    declaration: &SchedulingOutcomeDeclaration,
) -> Result<(), ContainedTaskError> {
    let designated_operation = declaration.designated_operation();
    let candidates = operations
        .iter()
        .map(|operation| RunOperationCandidate::new(&operation.id, &operation.from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
    let mut pending = observable_pages
        .iter()
        .map(|page| (page.clone(), SchedulingEffectCondition::NoDesignatedEffect))
        .collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();
    let mut reachable_terminals = BTreeSet::new();

    while let Some((page, condition)) = pending.pop_front() {
        if !visited.insert((page.clone(), condition)) {
            continue;
        }
        if target_pages
            .iter()
            .any(|target| crate::page_anchor_matches(game, &page, target))
        {
            reachable_terminals.insert((condition, page));
            continue;
        }
        if let Some(selected) = select_run_operation(game, &page, &candidates) {
            let operation = operations
                .iter()
                .find(|operation| operation.id == selected.id())
                .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
            if condition == SchedulingEffectCondition::DesignatedEffectCompleted
                && designated_operation == Some(operation.id.as_str())
            {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_outcome_declaration_incomplete",
                    format!(
                        "designated_operation={} is reachable after its effect completed",
                        operation.id
                    ),
                ));
            }
            let next_condition = if condition
                == SchedulingEffectCondition::DesignatedEffectCompleted
                || designated_operation == Some(operation.id.as_str())
            {
                SchedulingEffectCondition::DesignatedEffectCompleted
            } else {
                SchedulingEffectCondition::NoDesignatedEffect
            };
            let destinations = operation.destination_pages()?;
            if destinations.is_empty() {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_outcome_declaration_incomplete",
                    format!("operation={} has no finite postcondition", operation.id),
                ));
            }
            for destination in destinations {
                let matching_pages = observable_pages
                    .iter()
                    .filter(|page| crate::page_anchor_matches(game, page, &destination))
                    .collect::<Vec<_>>();
                let [concrete_page] = matching_pages.as_slice() else {
                    return Err(ContainedTaskError::with_detail(
                        "contained_task_outcome_declaration_incomplete",
                        format!(
                            "operation={} destination={} detector_matches={}",
                            operation.id,
                            destination,
                            matching_pages.len()
                        ),
                    ));
                };
                pending.push_back(((*concrete_page).clone(), next_condition));
            }
        }
    }

    for (condition, terminal_page) in reachable_terminals {
        let mapping_count = declaration
            .mappings()
            .iter()
            .filter(|mapping| {
                mapping.effect() == condition
                    && mapping.terminal_pages().iter().any(|mapped_page| {
                        crate::page_anchor_matches(game, &terminal_page, mapped_page)
                    })
            })
            .count();
        if mapping_count != 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_outcome_declaration_incomplete",
                format!(
                    "effect={condition:?} terminal_page={terminal_page} mappings={mapping_count}"
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PageDeclaration {
    Singleton(String),
    Set(Vec<String>),
}

impl PageDeclaration {
    fn normalized(&self) -> Result<Vec<String>, ContainedTaskError> {
        let pages = match self {
            Self::Singleton(page) => vec![page.clone()],
            Self::Set(pages) => pages.clone(),
        };
        if pages.is_empty() || pages.iter().any(|page| page.trim().is_empty()) {
            return Err(ContainedTaskError::new("contained_task_page_set_invalid"));
        }
        let unique = pages.iter().collect::<BTreeSet<_>>();
        if unique.len() != pages.len() {
            return Err(ContainedTaskError::new("contained_task_page_set_invalid"));
        }
        let mut pages = pages;
        pages.sort();
        Ok(pages)
    }
}

fn validate_page_references(
    game: &str,
    pages: &[String],
    detector: &PageDetector,
) -> Result<(), ContainedTaskError> {
    for page in pages {
        let matches = detector
            .page_ids()
            .filter(|candidate| crate::page_anchor_matches(game, candidate, page))
            .count();
        if matches != 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("page={page} detector_matches={matches}"),
            ));
        }
    }
    for candidate in detector.page_ids() {
        let matches = pages
            .iter()
            .filter(|page| crate::page_anchor_matches(game, candidate, page))
            .count();
        if matches > 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("detector_page={candidate} declaration_matches={matches}"),
            ));
        }
    }
    Ok(())
}

fn validate_page_set_overlap(
    game: &str,
    destinations: &[String],
    error_pages: &[String],
    detector: &PageDetector,
) -> Result<(), ContainedTaskError> {
    for candidate in detector.page_ids() {
        let destination = destinations
            .iter()
            .any(|page| crate::page_anchor_matches(game, candidate, page));
        let error = error_pages
            .iter()
            .any(|page| crate::page_anchor_matches(game, candidate, page));
        if destination && error {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("detector_page={candidate} is both destination and error"),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct TaskOperationDefaults {
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TaskOperationExpectation {
    page_id: PageDeclaration,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl TaskOperationExpectation {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        self.page_id.normalized()?;
        validate_bounded(self.timeout_ms, MAX_STEP_TIMEOUT_MS)?;
        validate_bounded(self.interval_ms, MAX_CAPTURE_INTERVAL_MS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TaskRecovery {
    Kind(String),
    Config {
        kind: String,
        #[serde(default)]
        task_id: Option<String>,
    },
}

impl TaskRecovery {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.kind() != "return_home" || self.task_id().trim().is_empty() {
            Err(ContainedTaskError::new("contained_task_recovery_invalid"))
        } else {
            Ok(())
        }
    }

    fn kind(&self) -> &str {
        match self {
            Self::Kind(kind) | Self::Config { kind, .. } => kind,
        }
    }

    fn task_id(&self) -> &str {
        match self {
            Self::Kind(_) => "return_home",
            Self::Config { task_id, .. } => task_id.as_deref().unwrap_or("return_home"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TaskOperation {
    id: String,
    from: String,
    #[serde(default)]
    to: Option<PageDeclaration>,
    #[serde(default)]
    expect_after: Option<TaskOperationExpectation>,
    click: TaskClick,
    #[serde(default)]
    on_error: Option<String>,
    #[serde(default)]
    retryable: Option<bool>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    post_delay_ms: Option<u64>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
}

impl TaskOperation {
    fn validate(
        &self,
        control: &TaskControl,
        defaults: TaskOperationDefaults,
    ) -> Result<(), ContainedTaskError> {
        if self.id.trim().is_empty() || self.from.trim().is_empty() {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        self.destination_pages()?;
        if let Some(expectation) = &self.expect_after {
            expectation.validate()?;
        }
        if self
            .on_error
            .as_deref()
            .is_some_and(|value| value != "return_home")
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        self.retry_policy(
            defaults,
            control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
        )?;
        if self
            .post_delay_ms
            .is_some_and(|value| value == 0 || value > MAX_CAPTURE_INTERVAL_MS)
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        match (&self.guard, self.unguarded_trusted_coordinate) {
            (Some(_), true) | (None, false) => {
                return Err(ContainedTaskError::new("contained_task_guard_missing"));
            }
            (Some(guard), false) => guard.validate(self, control)?,
            (None, true) => {}
        }
        self.click
            .validate(&control.resolution, self.guard.as_ref())
    }

    fn retry_policy(
        &self,
        defaults: TaskOperationDefaults,
        task_timeout_ms: u64,
    ) -> Result<Option<RunOperationPolicy>, ContainedTaskError> {
        let (retryable, max_attempts, retry_interval_ms) =
            match (self.retryable, self.max_attempts, self.retry_interval_ms) {
                (None, None, None) => return Ok(None),
                (Some(false), max_attempts, retry_interval_ms) => (
                    false,
                    max_attempts.or(defaults.max_attempts).unwrap_or(1),
                    retry_interval_ms
                        .or(defaults.retry_interval_ms)
                        .unwrap_or(1),
                ),
                (Some(true), max_attempts, retry_interval_ms) => (
                    true,
                    max_attempts.or(defaults.max_attempts).ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?,
                    retry_interval_ms
                        .or(defaults.retry_interval_ms)
                        .ok_or_else(|| {
                            ContainedTaskError::new("contained_task_operation_invalid")
                        })?,
                ),
                _ => {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            };
        if retryable
            && (self.destination_pages()?.is_empty() || retry_interval_ms > task_timeout_ms)
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        RunOperationPolicy::new(
            retryable,
            max_attempts,
            retry_interval_ms,
            self.on_error.clone(),
        )
        .map(Some)
        .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))
    }

    fn failure_decision(
        &self,
        policy: &RunOperationPolicy,
        attempt: u32,
        reason: &str,
        after_page: Option<String>,
        stage: RunFailureStage,
    ) -> Result<RunOperationFailureDecision, ContainedTaskError> {
        let observation = RunFailureObservation::new(&self.id, attempt, reason, after_page, stage)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        decide_run_operation_failure(policy, observation)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))
    }

    fn destination_pages(&self) -> Result<Vec<String>, ContainedTaskError> {
        let to = self
            .to
            .as_ref()
            .map(PageDeclaration::normalized)
            .transpose()?;
        let expected = self
            .expect_after
            .as_ref()
            .map(|expectation| expectation.page_id.normalized())
            .transpose()?;
        match (to, expected) {
            (Some(to), Some(expected)) if to != expected => Err(ContainedTaskError::new(
                "contained_task_destination_conflict",
            )),
            (Some(to), _) => Ok(to),
            (None, Some(expected)) => Ok(expected),
            (None, None) => Ok(Vec::new()),
        }
    }

    fn matching_destination_count(
        &self,
        control: &TaskControl,
        observation: &PageObservation,
    ) -> Result<usize, ContainedTaskError> {
        Ok(self
            .destination_pages()?
            .iter()
            .filter(|expected| {
                crate::page_anchor_matches(&control.game, &observation.page_label, expected)
            })
            .count())
    }

    fn guard_outcome(
        &self,
        control: &TaskControl,
        observation: &PageObservation,
        evaluator: &RecognitionEvaluator,
    ) -> Result<(ContainedTaskGuardOutcome, Option<TargetEvaluation>), ContainedTaskError> {
        if self.unguarded_trusted_coordinate {
            return Ok((ContainedTaskGuardOutcome::TrustedCoordinate, None));
        }
        let guard = self
            .guard
            .as_ref()
            .ok_or_else(|| ContainedTaskError::new("contained_task_guard_missing"))?;
        if !crate::page_anchor_matches(&control.game, &observation.page_label, &guard.page_id) {
            return Err(ContainedTaskError::with_detail(
                "contained_task_guard_refused",
                format!(
                    "operation={} expected_page={} observed_page={}",
                    self.id, guard.page_id, observation.page_label
                ),
            ));
        }
        let target = evaluator
            .evaluate_target(&observation.scene, &guard.target_id)
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    "contained_task_guard_evaluation_failed",
                    error.to_string(),
                )
            })?;
        if !target.passed {
            return Err(ContainedTaskError::with_detail(
                "contained_task_guard_refused",
                format!("operation={} target={}", self.id, guard.target_id),
            ));
        }
        let outcome = ContainedTaskGuardOutcome::Passed {
            page_label: observation.page_label.clone(),
            target_id: target.id.clone(),
            target_kind: target_kind_name(target.kind).to_string(),
        };
        Ok((outcome, Some(target)))
    }
}

#[derive(Debug, Deserialize)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: ClickRect,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    color_probe: Option<String>,
}

impl OperationGuard {
    fn validate(
        &self,
        operation: &TaskOperation,
        control: &TaskControl,
    ) -> Result<(), ContainedTaskError> {
        if self.page_id.trim().is_empty()
            || self.target_id.trim().is_empty()
            || !crate::page_anchor_matches(&control.game, &self.page_id, &operation.from)
            || (self.verify_template.is_none() && self.color_probe.is_none())
        {
            return Err(ContainedTaskError::new("contained_task_guard_invalid"));
        }
        self.expected_rect.validate(&control.resolution)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct TaskClick {
    kind: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    offset: Option<ClickRect>,
    #[serde(default)]
    from_rect: Option<ClickRect>,
    #[serde(default)]
    to_rect: Option<ClickRect>,
}

impl TaskClick {
    fn validate(
        &self,
        resolution: &Resolution,
        guard: Option<&OperationGuard>,
    ) -> Result<(), ContainedTaskError> {
        match self.kind.as_str() {
            "point" => {
                resolution.validate_point(required(self.x)?, required(self.y)?)?;
            }
            "rect" | "specific_rect" => ClickRect {
                x: required(self.x)?,
                y: required(self.y)?,
                width: required(self.width)?,
                height: required(self.height)?,
            }
            .validate(resolution)?,
            "long_press" | "long_tap" => {
                resolution.validate_point(required(self.x)?, required(self.y)?)?;
                if self.duration_ms == Some(0) || self.duration_ms.is_none() {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            }
            "drag" => {
                self.from_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                    .validate(resolution)?;
                self.to_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                    .validate(resolution)?;
                if self.duration_ms == Some(0) || self.duration_ms.is_none() {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            }
            "target" | "target_center" | "offset" => {
                let guard =
                    guard.ok_or_else(|| ContainedTaskError::new("contained_task_guard_missing"))?;
                if guard.verify_template.is_none()
                    || self
                        .target_id
                        .as_deref()
                        .is_some_and(|target_id| target_id != guard.target_id)
                {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
                if self.kind == "offset" {
                    self.offset
                        .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                        .validate_shape()?;
                } else if let Some(offset) = self.offset {
                    offset.validate_shape()?;
                }
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        }
        Ok(())
    }

    fn input_action(
        &self,
        resolution: &Resolution,
        target: Option<&TargetEvaluation>,
        action_seed: Option<u64>,
    ) -> Result<(InputAction, Option<InputSamplingEvidence>), ContainedTaskError> {
        let (action, sampling) = match self.kind.as_str() {
            "point" => (
                InputAction::Tap {
                    x: required(self.x)?,
                    y: required(self.y)?,
                },
                None,
            ),
            "rect" | "specific_rect" => {
                let rect = ClickRect {
                    x: required(self.x)?,
                    y: required(self.y)?,
                    width: required(self.width)?,
                    height: required(self.height)?,
                };
                rect.validate(resolution)?;
                sampled_tap(rect, action_seed)?
            }
            "long_press" | "long_tap" => (
                InputAction::LongTap {
                    x: required(self.x)?,
                    y: required(self.y)?,
                    duration_ms: self.duration_ms.ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?,
                },
                None,
            ),
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
                from.validate(resolution)?;
                to.validate(resolution)?;
                sampled_swipe(
                    from,
                    to,
                    self.duration_ms.ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?,
                    action_seed,
                )?
            }
            "target" | "target_center" | "offset" => {
                let target = target.ok_or_else(|| {
                    ContainedTaskError::new("contained_task_guard_target_missing")
                })?;
                let template = target.template.ok_or_else(|| {
                    ContainedTaskError::new("contained_task_guard_target_invalid")
                })?;
                let mut rect = ClickRect {
                    x: template.x,
                    y: template.y,
                    width: template.width,
                    height: template.height,
                };
                if self.kind == "offset" {
                    let offset = self.offset.ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?;
                    rect = ClickRect {
                        x: rect.x + offset.x,
                        y: rect.y + offset.y,
                        width: offset.width,
                        height: offset.height,
                    };
                } else if let Some(offset) = self.offset {
                    rect = ClickRect {
                        x: rect.x + offset.x,
                        y: rect.y + offset.y,
                        width: offset.width,
                        height: offset.height,
                    };
                }
                rect.validate(resolution)?;
                if self.kind == "target_center" {
                    (
                        InputAction::Tap {
                            x: rect.x + rect.width / 2,
                            y: rect.y + rect.height / 2,
                        },
                        None,
                    )
                } else {
                    sampled_tap(rect, action_seed)?
                }
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        };
        action
            .validate()
            .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
        match &action {
            InputAction::Tap { x, y } | InputAction::LongTap { x, y, .. } => {
                resolution.validate_point(*x, *y)?;
            }
            InputAction::Swipe { x1, y1, x2, y2, .. } => {
                resolution.validate_point(*x1, *y1)?;
                resolution.validate_point(*x2, *y2)?;
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        }
        Ok((action, sampling))
    }
}

fn target_kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Template => "template",
        TargetKind::Color => "color",
        TargetKind::ClickOnly => "click_only",
        TargetKind::Ocr => "ocr",
        TargetKind::Nn => "nn",
    }
}

fn contained_task_admission_error(error: ExecutionBundleError) -> ContainedTaskError {
    let code = match &error {
        ExecutionBundleError::Containment(ContainmentError::RecognitionPack {
            code: RecognitionPackErrorCode::VisionProviderMissing,
            ..
        }) => "contained_task_vision_provider_missing",
        ExecutionBundleError::Containment(ContainmentError::RecognitionPack {
            code:
                RecognitionPackErrorCode::VisionProviderFailure
                | RecognitionPackErrorCode::VisionProviderInvalidResponse,
            ..
        }) => "contained_task_vision_provider_failed",
        _ => "contained_task_admission_failed",
    };
    ContainedTaskError::with_detail(code, error.to_string())
}

fn required<T: Copy>(value: Option<T>) -> Result<T, ContainedTaskError> {
    value.ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))
}

fn sampled_tap(
    rect: ClickRect,
    action_seed: Option<u64>,
) -> Result<(InputAction, Option<InputSamplingEvidence>), ContainedTaskError> {
    let Some(action_seed) = action_seed else {
        return Ok((
            InputAction::Tap {
                x: rect.x + rect.width / 2,
                y: rect.y + rect.height / 2,
            },
            None,
        ));
    };
    let mut state = normalized_xorshift64_state(action_seed);
    let (x, y) = rect.sample(&mut state)?;
    let sampling = InputSamplingEvidence::new(action_seed, vec![rect.sampling_region()?])
        .map_err(|_| ContainedTaskError::new("contained_task_sampling_invalid"))?;
    Ok((InputAction::Tap { x, y }, Some(sampling)))
}

fn sampled_swipe(
    from: ClickRect,
    to: ClickRect,
    duration_ms: u64,
    action_seed: Option<u64>,
) -> Result<(InputAction, Option<InputSamplingEvidence>), ContainedTaskError> {
    let Some(action_seed) = action_seed else {
        return Ok((
            InputAction::Swipe {
                x1: from.x + from.width / 2,
                y1: from.y + from.height / 2,
                x2: to.x + to.width / 2,
                y2: to.y + to.height / 2,
                duration_ms,
            },
            None,
        ));
    };
    let mut state = normalized_xorshift64_state(action_seed);
    let (x1, y1) = from.sample(&mut state)?;
    let (x2, y2) = to.sample(&mut state)?;
    let sampling = InputSamplingEvidence::new(
        action_seed,
        vec![from.sampling_region()?, to.sampling_region()?],
    )
    .map_err(|_| ContainedTaskError::new("contained_task_sampling_invalid"))?;
    Ok((
        InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        },
        Some(sampling),
    ))
}

fn normalized_xorshift64_state(seed: u64) -> u64 {
    if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    }
}

fn next_xorshift64(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct ClickRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl ClickRect {
    fn validate_shape(&self) -> Result<(), ContainedTaskError> {
        if self.width <= 0 || self.height <= 0 {
            Err(ContainedTaskError::new("contained_task_operation_invalid"))
        } else {
            Ok(())
        }
    }

    fn validate(&self, resolution: &Resolution) -> Result<(), ContainedTaskError> {
        self.validate_shape()?;
        let end_x = self
            .x
            .checked_add(self.width - 1)
            .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
        let end_y = self
            .y
            .checked_add(self.height - 1)
            .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
        resolution.validate_point(self.x, self.y)?;
        resolution.validate_point(end_x, end_y)
    }

    fn sampling_region(&self) -> Result<InputSamplingRegion, ContainedTaskError> {
        InputSamplingRegion::new(self.x, self.y, self.width, self.height)
            .map_err(|_| ContainedTaskError::new("contained_task_sampling_invalid"))
    }

    fn sample(&self, state: &mut u64) -> Result<(i32, i32), ContainedTaskError> {
        self.validate_shape()?;
        let x_offset = next_xorshift64(state) % self.width as u64;
        let y_offset = next_xorshift64(state) % self.height as u64;
        let x = i64::from(self.x) + x_offset as i64;
        let y = i64::from(self.y) + y_offset as i64;
        Ok((
            i32::try_from(x)
                .map_err(|_| ContainedTaskError::new("contained_task_sampling_invalid"))?,
            i32::try_from(y)
                .map_err(|_| ContainedTaskError::new("contained_task_sampling_invalid"))?,
        ))
    }
}

fn scene_from_frame(frame: &Frame) -> Result<Scene, ContainedTaskError> {
    let format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, format)
        .map_err(|_| ContainedTaskError::new("contained_task_frame_invalid"))
}

#[cfg(test)]
mod post_admission_ocr_tests {
    use super::*;
    use actingcommand_recognition_pack::{
        FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
        OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
        PackRect, RecognitionPack, VisionProviderError,
    };
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct EvidenceProvider {
        observations: Mutex<VecDeque<OcrProviderObservation>>,
        requests: Mutex<Vec<PackRect>>,
        calls: AtomicU32,
    }

    impl VisionProvider for EvidenceProvider {
        fn require_ocr_model(
            &self,
            model_ref: &str,
            model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            assert_eq!(model_ref, "PP-OCRv6_medium");
            assert_eq!(model_sha256, "a".repeat(64));
            Ok(())
        }

        fn require_nn_model(
            &self,
            _model_ref: &str,
            _model_sha256: &str,
        ) -> Result<(), VisionProviderError> {
            unreachable!("fixture exposes OCR only")
        }

        fn read_text(
            &self,
            _request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            unreachable!("post-admission path must request execution evidence")
        }

        fn read_text_with_execution_evidence(
            &self,
            request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderObservation, VisionProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests
                .lock()
                .expect("fixture request lock")
                .push(request.region);
            let mut observation = self
                .observations
                .lock()
                .expect("fixture provider lock")
                .pop_front()
                .ok_or_else(|| {
                    VisionProviderError::new(
                        actingcommand_recognition_pack::VisionProviderErrorCode::Internal,
                        "fixture observation missing",
                    )
                })?;
            for block in &mut observation.result.blocks {
                block.rect = request.region;
            }
            Ok(observation)
        }

        fn classify(
            &self,
            _request: NnProviderRequest<'_>,
        ) -> Result<NnProviderResult, VisionProviderError> {
            unreachable!("fixture exposes OCR only")
        }
    }

    fn execution(invocation_id: &str) -> OcrProviderExecutionEvidence {
        OcrProviderExecutionEvidence {
            invocation_id: invocation_id.to_string(),
            session_id: "session-1".to_string(),
            session_generation: 1,
            requested_provider: OcrExecutionProviderKind::Cpu,
            resolved_provider: OcrExecutionProviderKind::Cpu,
            requested_cuda_ordinal: None,
            requested_cuda_identity: None,
            resolved_cuda_ordinal: None,
            resolved_cuda_identity: None,
            provider_implementation: "fixture-ocr".to_string(),
            provider_binary_sha256: "b".repeat(64),
            runtime_version: "fixture-runtime".to_string(),
            model_ref: "PP-OCRv6_medium".to_string(),
            model_sha256: "a".repeat(64),
            cpu_ep_registered: true,
            cpu_fallback_disabled: false,
            fallback_forbidden: true,
            fallback_observed: None,
            complete: true,
        }
    }

    fn provider_observation(invocation_id: &str, values: &[String]) -> OcrProviderObservation {
        OcrProviderObservation {
            result: OcrProviderResult {
                text: values.join("\n"),
                blocks: values
                    .iter()
                    .map(|value| OcrProviderTextBlock {
                        text: value.clone(),
                        rect: PackRect {
                            x: 0,
                            y: 0,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.9),
                    })
                    .collect(),
                confidence: Some(0.9),
            },
            execution: Some(execution(invocation_id)),
        }
    }

    fn evaluator(provider: Arc<EvidenceProvider>) -> RecognitionEvaluator {
        let pack: RecognitionPack = serde_json::from_value(serde_json::json!({
            "schema_version": "0.6",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 1, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [{
                "type": "ocr",
                "id": "fixture/ocr",
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "languages": ["en"],
                "timeout_ms": 1000,
                "match_mode": "exact",
                "expected": ["unused"],
                "case_sensitive": true,
                "minimum_confidence": 0.0,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }]
        }))
        .expect("fixture recognition pack");
        RecognitionEvaluator::with_vision_provider(
            pack,
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            provider,
        )
        .expect("fixture evaluator")
    }

    fn ordered_target_ids() -> Vec<String> {
        (0..16)
            .map(|index| format!("fixture/ocr-{index:02}"))
            .collect()
    }

    fn ordered_ocr_pack(target_ids: &[String], coordinate_width: u32) -> RecognitionPack {
        let mut targets = vec![
            serde_json::json!({
                "type": "color",
                "id": "page/operator",
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "expected": [1, 1, 1]
            }),
            serde_json::json!({
                "type": "color",
                "id": "page/operator_end",
                "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                "expected": [2, 2, 2]
            }),
        ];
        targets.extend(target_ids.iter().enumerate().map(|(index, target_id)| {
            serde_json::json!({
                "type": "ocr",
                "id": target_id,
                "region": {"x": index, "y": 0, "width": 1, "height": 1},
                "languages": ["en"],
                "timeout_ms": 1000,
                "match_mode": "exact",
                "expected": ["unused"],
                "case_sensitive": true,
                "minimum_confidence": 0.0,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "a".repeat(64)
            })
        }));
        serde_json::from_value(serde_json::json!({
            "schema_version": "0.6",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": coordinate_width, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": targets
        }))
        .expect("ordered OCR recognition pack")
    }

    fn prepared_ordered(target_ids: Vec<String>, truth: Vec<String>) -> PreparedPostAdmissionOcr {
        PreparedPostAdmissionOcr {
            declaration: PostAdmissionOcrDeclaration {
                page_id: None,
                page_ids: Some(vec!["operator".to_string(), "operator_end".to_string()]),
                target_id: None,
                target_ids: Some(target_ids.clone()),
                truth_set: PostAdmissionOcrTruthReference {
                    path: "truth.json".to_string(),
                    sha256: "c".repeat(64),
                },
                normalization: PostAdmissionOcrNormalization::TrimLowercaseV1,
                comparison: PostAdmissionOcrComparisonMode::ExactSetV1,
                limits: PostAdmissionOcrLimits {
                    max_frames: 2,
                    max_items: 64,
                    max_string_bytes: 64,
                    max_total_bytes: 16_384,
                    max_truth_entries: 64,
                },
                outcome_key: "comparison_recorded".to_string(),
            },
            page_ids: vec!["operator".to_string(), "operator_end".to_string()],
            target_ids,
            truth,
        }
    }

    fn prepared(truth: Vec<String>) -> PreparedPostAdmissionOcr {
        PreparedPostAdmissionOcr {
            declaration: PostAdmissionOcrDeclaration {
                page_id: Some("target".to_string()),
                page_ids: None,
                target_id: Some("fixture/ocr".to_string()),
                target_ids: None,
                truth_set: PostAdmissionOcrTruthReference {
                    path: "truth.json".to_string(),
                    sha256: "c".repeat(64),
                },
                normalization: PostAdmissionOcrNormalization::TrimLowercaseV1,
                comparison: PostAdmissionOcrComparisonMode::ExactSetV1,
                limits: PostAdmissionOcrLimits {
                    max_frames: 2,
                    max_items: 500,
                    max_string_bytes: 64,
                    max_total_bytes: 1_000_000,
                    max_truth_entries: 500,
                },
                outcome_key: "comparison_recorded".to_string(),
            },
            page_ids: vec!["target".to_string()],
            target_ids: vec!["fixture/ocr".to_string()],
            truth,
        }
    }

    fn collect_422_name_report() -> (PostAdmissionOcrComparisonReport, u32) {
        let truth = (0..422)
            .map(|index| format!("name-{index:03}"))
            .collect::<Vec<_>>();
        let mut first = truth[..211].to_vec();
        first[0] = format!("  {}  ", first[0].to_uppercase());
        let mut second = truth[211..].to_vec();
        second.push("NAME-000".to_string());
        let provider = Arc::new(EvidenceProvider {
            observations: Mutex::new(VecDeque::from([
                provider_observation("invocation-1", &first),
                provider_observation("invocation-2", &second),
            ])),
            requests: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
        });
        let evaluator = evaluator(Arc::clone(&provider));
        let prepared = prepared(truth);
        let scene = scene_from_frame(
            &Frame::from_pixels(
                1,
                1,
                vec![0, 0, 0],
                PixelFormat::Rgb8,
                actingcommand_device::CaptureBackendName::FixtureSimulation,
            )
            .expect("fixture frame"),
        )
        .expect("fixture scene");
        let mut collector = PostAdmissionOcrCollector::new(Some(&prepared));
        assert!(
            collector
                .observe("neutral", &evaluator, "neutral/other", &scene)
                .expect("non-admitted page")
                .is_none()
        );
        for frame_index in 0..2 {
            let (observed_index, observation) = collector
                .observe("neutral", &evaluator, "neutral/target", &scene)
                .expect("admitted OCR observation")
                .expect("bounded observation");
            assert_eq!(observed_index, frame_index);
            assert_eq!(observation.target_id.as_deref(), Some("fixture/ocr"));
            assert!(observation.targets.is_none());
            assert!(
                serde_json::to_value(&observation)
                    .expect("legacy observation evidence")
                    .get("targets")
                    .is_none()
            );
        }
        let report = collector.finish().expect("comparison").expect("report");
        (report, provider.calls.load(Ordering::SeqCst))
    }

    #[test]
    fn post_admission_ocr_collects_one_bounded_stream_and_compares_422_names() {
        let (first, calls) = collect_422_name_report();
        let (second, repeated_calls) = collect_422_name_report();

        assert_eq!(calls, 2, "non-admitted page must not invoke OCR");
        assert_eq!(repeated_calls, 2);
        assert_eq!(first.target_id.as_deref(), Some("fixture/ocr"));
        assert!(first.target_ids.is_none());
        assert!(
            serde_json::to_value(&first)
                .expect("legacy singleton evidence")
                .get("target_ids")
                .is_none(),
            "legacy singleton serialization must not gain target_ids"
        );
        assert_eq!(first.frames_collected, 2);
        assert_eq!(first.items_collected, 423);
        assert_eq!(first.truth.len(), 422);
        assert_eq!(first.observed.len(), 422);
        assert!(first.exact_match);
        assert!(first.missed.is_empty());
        assert!(first.unexpected.is_empty());
        assert_eq!(
            first.duplicates,
            vec![PostAdmissionOcrDuplicateEvidence {
                value: "name-000".to_string(),
                occurrences: 2,
            }]
        );
        assert_eq!(
            serde_json::to_vec(&first).expect("first report bytes"),
            serde_json::to_vec(&second).expect("second report bytes")
        );
    }

    #[test]
    fn post_admission_ocr_evaluates_sixteen_targets_on_each_declared_page_in_order() {
        let target_ids = ordered_target_ids();
        let values = target_ids
            .iter()
            .enumerate()
            .map(|(index, _)| format!("operator-{index:02}"))
            .collect::<Vec<_>>();
        let mut provider_observations = Vec::new();
        for page_index in 0..2 {
            for (target_index, value) in values.iter().enumerate() {
                provider_observations.push(provider_observation(
                    &format!("ordered-invocation-{page_index}-{target_index:02}"),
                    std::slice::from_ref(value),
                ));
            }
        }
        let provider = Arc::new(EvidenceProvider {
            observations: Mutex::new(VecDeque::from(provider_observations)),
            requests: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
        });
        let evaluator = RecognitionEvaluator::with_vision_provider(
            ordered_ocr_pack(&target_ids, 16),
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            provider.clone(),
        )
        .expect("ordered evaluator");
        let prepared = prepared_ordered(target_ids.clone(), values.clone());
        let scene = scene_from_frame(
            &Frame::from_pixels(
                16,
                1,
                vec![0; 16 * 3],
                PixelFormat::Rgb8,
                actingcommand_device::CaptureBackendName::FixtureSimulation,
            )
            .expect("ordered fixture frame"),
        )
        .expect("ordered fixture scene");
        let mut collector = PostAdmissionOcrCollector::new(Some(&prepared));

        assert!(
            collector
                .observe("neutral", &evaluator, "neutral/other", &scene)
                .expect("non-admitted page")
                .is_none()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        for (frame_index, page_label) in ["neutral/operator", "neutral/operator_end"]
            .into_iter()
            .enumerate()
        {
            let (observed_frame_index, observation) = collector
                .observe("neutral", &evaluator, page_label, &scene)
                .expect("ordered observation")
                .expect("admitted frame");
            assert_eq!(observed_frame_index, frame_index as u32);
            assert!(observation.target_id.is_none());
            let observations = observation
                .targets
                .as_ref()
                .expect("ordered target evidence");
            assert_eq!(
                observations
                    .iter()
                    .map(|observation| observation.target_id.as_str())
                    .collect::<Vec<_>>(),
                target_ids.iter().map(String::as_str).collect::<Vec<_>>()
            );
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 32);
        assert_eq!(
            provider
                .requests
                .lock()
                .expect("ordered requests")
                .iter()
                .map(|region| region.x)
                .collect::<Vec<_>>(),
            (0..16).chain(0..16).collect::<Vec<_>>()
        );
        let report = collector.finish().expect("comparison").expect("report");
        assert_eq!(report.frames_collected, 2);
        assert_eq!(report.items_collected, 32);
        assert!(report.exact_match);
        assert!(report.target_id.is_none());
        assert_eq!(report.target_ids.as_ref(), Some(&target_ids));
        let serialized = serde_json::to_value(&report).expect("ordered report evidence");
        assert!(serialized.get("target_id").is_none());
        assert_eq!(serialized["target_ids"], json!(target_ids));
    }

    #[test]
    fn post_admission_ocr_page_and_target_forms_fail_closed_before_provider_access() {
        let target_ids = ordered_target_ids();
        let base = serde_json::json!({
            "page_id": "target",
            "truth_set": {"path": "truth.json", "sha256": "c".repeat(64)},
            "normalization": "trim_lowercase_v1",
            "comparison": "exact_set_v1",
            "limits": {
                "max_frames": 2,
                "max_items": 64,
                "max_string_bytes": 64,
                "max_total_bytes": 16_384,
                "max_truth_entries": 64
            },
            "outcome_key": "comparison_recorded"
        });
        let declaration = |target_fields: Value| {
            let mut value = base.clone();
            value
                .as_object_mut()
                .expect("declaration object")
                .extend(target_fields.as_object().expect("target fields").clone());
            serde_json::from_value::<PostAdmissionOcrDeclaration>(value)
        };

        declaration(json!({"target_id": "fixture/ocr"}))
            .expect("legacy target form")
            .validate()
            .expect("legacy target validation");
        declaration(json!({"target_ids": target_ids.clone()}))
            .expect("ordered target form")
            .validate()
            .expect("ordered target validation");
        for invalid in [
            json!({}),
            json!({"target_id": "fixture/ocr", "target_ids": ["fixture/ocr"]}),
            json!({"target_ids": []}),
            json!({"target_ids": ["fixture/ocr", "fixture/ocr"]}),
            json!({"target_ids": (0..33).map(|index| format!("ocr/{index}"))
                .collect::<Vec<_>>() }),
        ] {
            let declaration = declaration(invalid).expect("target form parses");
            assert_eq!(
                declaration
                    .validate()
                    .expect_err("invalid target form must fail closed")
                    .code(),
                "contained_task_post_admission_ocr_invalid"
            );
        }
        assert!(declaration(json!({"target_ids": null})).is_err());

        let page_declaration = |page_fields: Value| {
            let mut value = base.clone();
            value
                .as_object_mut()
                .expect("declaration object")
                .remove("page_id");
            value
                .as_object_mut()
                .expect("declaration object")
                .extend(page_fields.as_object().expect("page fields").clone());
            value["target_id"] = json!("fixture/ocr");
            serde_json::from_value::<PostAdmissionOcrDeclaration>(value)
        };
        page_declaration(json!({"page_id": "target"}))
            .expect("legacy page form")
            .validate()
            .expect("legacy page validation");
        page_declaration(json!({"page_ids": ["operator", "operator_end"]}))
            .expect("exact two-page form")
            .validate()
            .expect("exact two-page validation");
        for invalid in [
            json!({}),
            json!({"page_id": "operator", "page_ids": ["operator", "operator_end"]}),
            json!({"page_ids": []}),
            json!({"page_ids": ["operator"]}),
            json!({"page_ids": ["operator", "operator"]}),
            json!({"page_ids": ["operator", "operator_end", "other"]}),
            json!({"page_ids": ["operator", ""]}),
        ] {
            let declaration = page_declaration(invalid).expect("page form parses");
            assert_eq!(
                declaration
                    .validate()
                    .expect_err("invalid page form must fail closed")
                    .code(),
                "contained_task_post_admission_ocr_invalid"
            );
        }
        assert!(page_declaration(json!({"page_ids": null})).is_err());

        let control: TaskControl = serde_json::from_value(json!({
            "schema_version": CONTROL_SCHEMA,
            "package_id": "neutral.test.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 1},
            "entry_task_id": "task"
        }))
        .expect("target validation control");
        let evaluator = RecognitionEvaluator::with_asset_resolver(
            ordered_ocr_pack(&target_ids, 16),
            Arc::new(FsAssetResolver::new(PathBuf::new())),
        )
        .expect("provider-free ordered evaluator");
        for target_id in &target_ids {
            validate_post_admission_ocr_target(&control, &evaluator, target_id)
                .expect("in-bounds OCR target");
        }
        assert_eq!(
            validate_post_admission_ocr_target(&control, &evaluator, "missing")
                .expect_err("unknown target must fail closed")
                .code(),
            "contained_task_post_admission_ocr_target_invalid"
        );

        let mut off_page_ids = (0..17)
            .map(|index| format!("fixture/off-page-{index:02}"))
            .collect::<Vec<_>>();
        off_page_ids[16] = "fixture/off-page".to_string();
        let off_page = RecognitionEvaluator::with_asset_resolver(
            ordered_ocr_pack(&off_page_ids, 17),
            Arc::new(FsAssetResolver::new(PathBuf::new())),
        )
        .expect("off-page evaluator remains valid in its own coordinate space");
        assert_eq!(
            validate_post_admission_ocr_target(&control, &off_page, "fixture/off-page")
                .expect_err("target outside admitted page bounds must fail closed")
                .code(),
            "contained_task_post_admission_ocr_target_out_of_bounds"
        );

        let non_ocr_pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.6",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 16, "height": 1},
            "targets": [{
                "type": "color",
                "id": "fixture/not-ocr",
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "expected": [0, 0, 0]
            }]
        }))
        .expect("non-OCR pack");
        let non_ocr = RecognitionEvaluator::with_asset_resolver(
            non_ocr_pack,
            Arc::new(FsAssetResolver::new(PathBuf::new())),
        )
        .expect("non-OCR evaluator");
        assert_eq!(
            validate_post_admission_ocr_target(&control, &non_ocr, "fixture/not-ocr")
                .expect_err("non-OCR target must fail closed")
                .code(),
            "contained_task_post_admission_ocr_target_invalid"
        );
    }

    #[test]
    fn post_admission_ocr_two_page_gates_are_unique_positive_and_non_ocr() {
        let target_ids = ordered_target_ids();
        let evaluator = RecognitionEvaluator::with_asset_resolver(
            ordered_ocr_pack(&target_ids, 16),
            Arc::new(FsAssetResolver::new(PathBuf::new())),
        )
        .expect("provider-free page-gate evaluator");
        let control: TaskControl = serde_json::from_value(json!({
            "schema_version": CONTROL_SCHEMA,
            "package_id": "neutral.test.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 1},
            "entry_task_id": "task"
        }))
        .expect("page-gate control");
        let pages = |entries: Value| {
            serde_json::from_value::<PageSet>(json!({
                "schema_version": "0.3",
                "pages": entries
            }))
            .expect("page set")
        };
        let good = pages(json!([
            {"id": "neutral/operator", "required": ["page/operator"]},
            {"id": "neutral/operator_end", "required": ["page/operator_end"]}
        ]));
        let page_ids = vec!["operator".to_string(), "operator_end".to_string()];
        validate_page_references(
            &control.game,
            &page_ids,
            &PageDetector::new(good.clone()).unwrap(),
        )
        .expect("each page resolves exactly once");
        validate_post_admission_ocr_page_set(&control, &evaluator, &good, &page_ids, &target_ids)
            .expect("two positive color-only page gates");

        assert_eq!(
            validate_post_admission_ocr_page_set(
                &control,
                &evaluator,
                &good,
                &["operator".to_string(), "missing".to_string()],
                &target_ids,
            )
            .expect_err("unknown page must fail closed")
            .code(),
            "contained_task_post_admission_ocr_page_gate_invalid"
        );
        let duplicate = pages(json!([
            {"id": "neutral/operator", "required": ["page/operator"]},
            {"id": "neutral/operator", "required": ["page/operator"]},
            {"id": "neutral/operator_end", "required": ["page/operator_end"]}
        ]));
        assert_eq!(
            validate_post_admission_ocr_page_set(
                &control,
                &evaluator,
                &duplicate,
                &page_ids,
                &target_ids,
            )
            .expect_err("duplicate page resolution must fail closed")
            .code(),
            "contained_task_post_admission_ocr_page_gate_invalid"
        );
        let empty_positive = pages(json!([
            {"id": "neutral/operator", "required": []},
            {"id": "neutral/operator_end", "required": ["page/operator_end"]}
        ]));
        assert_eq!(
            validate_post_admission_ocr_page_set(
                &control,
                &evaluator,
                &empty_positive,
                &page_ids,
                &target_ids,
            )
            .expect_err("each page requires a positive gate")
            .code(),
            "contained_task_post_admission_ocr_page_gate_invalid"
        );
        let ocr_gate = pages(json!([
            {"id": "neutral/operator", "required": ["fixture/ocr-00"]},
            {"id": "neutral/operator_end", "required": ["page/operator_end"]}
        ]));
        assert_eq!(
            validate_post_admission_ocr_page_set(
                &control,
                &evaluator,
                &ocr_gate,
                &page_ids,
                &target_ids,
            )
            .expect_err("selected OCR target must stay out of every page gate")
            .code(),
            "contained_task_post_admission_ocr_target_in_page_gate"
        );
    }

    #[test]
    fn schema_0_7_task_timeout_exactly_matches_the_validated_control() {
        let control = |timeout_ms: Option<u64>| {
            let mut value = json!({
                "schema_version": CONTROL_SCHEMA,
                "package_id": "neutral.test.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 1, "height": 1},
                "entry_task_id": "task"
            });
            if let Some(timeout_ms) = timeout_ms {
                value["timeout_ms"] = json!(timeout_ms);
            }
            serde_json::from_value::<TaskControl>(value).expect("task timeout control")
        };
        let program = |schema_version: &str, timeout_ms: Option<Value>| {
            let mut value = json!({
                "schema_version": schema_version,
                "task_id": "task",
                "game": "neutral",
                "coordinate_space": {"width": 1, "height": 1},
                "operations": []
            });
            if schema_version == "0.7" {
                value["post_admission_ocr"] = json!({
                    "page_id": "target",
                    "target_id": "fixture/ocr",
                    "truth_set": {"path": "truth.json", "sha256": "c".repeat(64)},
                    "normalization": "trim_lowercase_v1",
                    "comparison": "exact_set_v1",
                    "limits": {
                        "max_frames": 1,
                        "max_items": 1,
                        "max_string_bytes": 64,
                        "max_total_bytes": 4096,
                        "max_truth_entries": 1
                    },
                    "outcome_key": "comparison_recorded"
                });
            }
            if let Some(timeout_ms) = timeout_ms {
                value["timeout_ms"] = timeout_ms;
            }
            serde_json::from_value::<TaskProgram>(value)
        };

        let absent = program("0.7", None).expect("absent timeout program");
        absent
            .validate_task_timeout(&control(Some(5_000)))
            .expect("absence preserves the existing control timeout");
        let bounded = program("0.7", Some(json!(300_000))).expect("bounded timeout program");
        bounded
            .validate_task_timeout(&control(Some(300_000)))
            .expect("matching bounded timeout");
        assert_eq!(
            bounded
                .validate_task_timeout(&control(Some(299_999)))
                .expect_err("timeout mismatch must fail closed")
                .code(),
            "contained_task_program_invalid"
        );
        assert!(program("0.7", Some(Value::Null)).is_err());
        assert_eq!(
            program("0.6", Some(json!(300_000)))
                .expect("legacy timeout parses for explicit validation")
                .validate_task_timeout(&control(None))
                .expect_err("legacy schema cannot declare task timeout")
                .code(),
            "contained_task_program_invalid"
        );
        for invalid in [0, 600_001] {
            assert_eq!(
                control(Some(invalid))
                    .validate()
                    .expect_err("control timeout must remain bounded")
                    .code(),
                "contained_task_control_invalid"
            );
        }
    }

    #[test]
    fn schema_0_7_max_steps_exactly_matches_the_validated_control() {
        let control = |max_steps: Option<u32>| {
            let mut value = json!({
                "schema_version": CONTROL_SCHEMA,
                "package_id": "neutral.test.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 1, "height": 1},
                "entry_task_id": "task"
            });
            if let Some(max_steps) = max_steps {
                value["max_steps"] = json!(max_steps);
            }
            serde_json::from_value::<TaskControl>(value).expect("max-steps control")
        };
        let program = |schema_version: &str, max_steps: Option<Value>| {
            let mut value = json!({
                "schema_version": schema_version,
                "task_id": "task",
                "game": "neutral",
                "coordinate_space": {"width": 1, "height": 1},
                "operations": []
            });
            if schema_version == "0.7" {
                value["post_admission_ocr"] = json!({
                    "page_id": "target",
                    "target_id": "fixture/ocr",
                    "truth_set": {"path": "truth.json", "sha256": "c".repeat(64)},
                    "normalization": "trim_lowercase_v1",
                    "comparison": "exact_set_v1",
                    "limits": {
                        "max_frames": 1,
                        "max_items": 1,
                        "max_string_bytes": 64,
                        "max_total_bytes": 4096,
                        "max_truth_entries": 1
                    },
                    "outcome_key": "comparison_recorded"
                });
            }
            if let Some(max_steps) = max_steps {
                value["max_steps"] = max_steps;
            }
            serde_json::from_value::<TaskProgram>(value)
        };

        program("0.7", None)
            .expect("absent max steps")
            .validate_task_max_steps(&control(None))
            .expect("absence preserves default behavior");
        for max_steps in [1_u32, 61, 1_000] {
            program("0.7", Some(json!(max_steps)))
                .expect("bounded max steps")
                .validate_task_max_steps(&control(Some(max_steps)))
                .expect("matching bounded max steps");
        }
        assert_eq!(
            program("0.7", Some(json!(61)))
                .expect("mismatch program")
                .validate_task_max_steps(&control(Some(62)))
                .expect_err("max-steps mismatch must fail closed")
                .code(),
            "contained_task_program_invalid"
        );
        for invalid in [json!(-1), json!(1.5), json!("61"), Value::Null] {
            assert!(program("0.7", Some(invalid)).is_err());
        }
        for invalid in [0, 1_001] {
            assert_eq!(
                program("0.7", Some(json!(invalid)))
                    .expect("integer max steps")
                    .validate_task_max_steps(&control(Some(invalid)))
                    .expect_err("out-of-range max steps must fail closed")
                    .code(),
                "contained_task_program_invalid"
            );
        }
        assert_eq!(
            program("0.6", Some(json!(61)))
                .expect("legacy max steps parses for explicit validation")
                .validate_task_max_steps(&control(Some(61)))
                .expect_err("legacy schema cannot declare max steps")
                .code(),
            "contained_task_program_invalid"
        );
    }

    #[test]
    fn stability_success_finishes_one_ocr_comparison_and_selects_the_owned_outcome() {
        struct StabilityOcrRuntime {
            frames: VecDeque<Frame>,
            last_frame: Frame,
            inputs: usize,
            traces: Vec<ContainedTaskTrace>,
        }

        impl ContainedTaskRuntime for StabilityOcrRuntime {
            type Error = &'static str;

            fn capture(&mut self) -> Result<Frame, Self::Error> {
                Ok(match self.frames.pop_front() {
                    Some(frame) => frame,
                    None => self.last_frame.clone(),
                })
            }

            fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
                self.inputs += 1;
                Ok(())
            }

            fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
                self.traces.push(trace);
                Ok(())
            }
        }

        let stability = serde_json::json!({
            "region": {"x": 1, "y": 0, "width": 1, "height": 1},
            "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
            "consecutive_unchanged_threshold": 2,
            "max_steps": 4
        });
        let control: TaskControl = serde_json::from_value(serde_json::json!({
            "schema_version": CONTROL_SCHEMA,
            "package_id": "neutral.test.task",
            "execution_mode": "in_page_guard",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "max_steps": 4,
            "stability_termination": stability.clone()
        }))
        .expect("stability control");
        control.validate().expect("valid stability control");
        let program: TaskProgram = serde_json::from_value(serde_json::json!({
            "schema_version": "0.7",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "scheduling_outcome": {
                "mappings": [{
                    "outcome_key": "comparison_recorded",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["home"]
                }]
            },
            "post_admission_ocr": {
                "page_id": "home",
                "target_id": "fixture/ocr",
                "truth_set": {"path": "truth.json", "sha256": "c".repeat(64)},
                "normalization": "trim_lowercase_v1",
                "comparison": "exact_set_v1",
                "limits": {
                    "max_frames": 2,
                    "max_items": 500,
                    "max_string_bytes": 64,
                    "max_total_bytes": 1_000_000,
                    "max_truth_entries": 500
                },
                "outcome_key": "comparison_recorded"
            },
            "stability_termination": stability,
            "operations": [{
                "id": "swipe_once",
                "from": "home",
                "to": "home",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true
            }]
        }))
        .expect("stability OCR program");
        validate_stability_contract(&control, &program)
            .expect("the OCR declaration owns its selected outcome");

        let provider = Arc::new(EvidenceProvider {
            observations: Mutex::new(VecDeque::from([
                provider_observation("stability-invocation-1", &["synthetic truth".to_string()]),
                provider_observation("stability-invocation-2", &["synthetic truth".to_string()]),
            ])),
            requests: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
        });
        let pack: RecognitionPack = serde_json::from_value(serde_json::json!({
            "schema_version": "0.6",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {
                    "type": "color",
                    "id": "page/home",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 0, 0]
                },
                {
                    "type": "ocr",
                    "id": "fixture/ocr",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "languages": ["en"],
                    "timeout_ms": 1000,
                    "match_mode": "exact",
                    "expected": ["unused"],
                    "case_sensitive": true,
                    "minimum_confidence": 0.0,
                    "model_ref": "PP-OCRv6_medium",
                    "model_sha256": "a".repeat(64)
                }
            ]
        }))
        .expect("stability recognition pack");
        let evaluator = RecognitionEvaluator::with_vision_provider(
            pack,
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            provider.clone(),
        )
        .expect("stability evaluator");
        let page_set: PageSet = serde_json::from_value(serde_json::json!({
            "schema_version": "0.3",
            "pages": [{
                "id": "neutral/home",
                "required": ["page/home"],
                "optional": [],
                "forbidden": []
            }]
        }))
        .expect("stability page set");
        let detector = PageDetector::new(page_set).expect("stability page detector");
        detector
            .validate(&evaluator)
            .expect("stability page detector targets");
        let scheduling_outcome = program.scheduling_outcome.clone();
        let mut post_admission_ocr = prepared(vec!["synthetic truth".to_string()]);
        post_admission_ocr.declaration.page_id = Some("home".to_string());
        post_admission_ocr.page_ids = vec!["home".to_string()];
        let task = PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            scheduling_outcome,
            post_admission_ocr: Some(post_admission_ocr),
            package_sha256: "fixture-sha256".to_string(),
            entry_count: 6,
            task_count: 1,
        };
        let frame = |sample: [u8; 3]| {
            Frame::from_pixels(
                2,
                1,
                [[255, 0, 0], sample].concat(),
                PixelFormat::Rgb8,
                actingcommand_device::CaptureBackendName::FixtureSimulation,
            )
            .expect("stability frame")
        };
        let final_frame = frame([10, 10, 10]);
        let mut runtime = StabilityOcrRuntime {
            frames: VecDeque::from([
                frame([1, 1, 1]),
                final_frame.clone(),
                final_frame.clone(),
                final_frame.clone(),
            ]),
            last_frame: final_frame,
            inputs: 0,
            traces: Vec::new(),
        };

        let outcome = task
            .run(&mut runtime)
            .expect("stability success with OCR selection");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.executed_steps, 3);
        assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
        assert_eq!(
            outcome.selected_scheduling_outcome.as_deref(),
            Some("comparison_recorded")
        );
        assert_eq!(runtime.inputs, 3);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(
                    trace,
                    ContainedTaskTrace::PostAdmissionOcrObservation { .. }
                ))
                .count(),
            2
        );
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(
                    trace,
                    ContainedTaskTrace::PostAdmissionOcrComparison { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::StabilityTerminal { .. }))
                .count(),
            1
        );
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }))
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod retry_wiring_tests {
    use super::*;
    use actingcommand_device::CaptureBackendName;
    use actingcommand_page_detector::PageSet;
    use actingcommand_recognition_pack::RecognitionPack;
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::time::Instant;

    fn control() -> TaskControl {
        serde_json::from_value(json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.semantic.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 2
        }))
        .expect("task control")
    }

    fn operation(retry: Value, on_error: Option<&str>) -> TaskOperation {
        let mut value = json!({
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        });
        if let Value::Object(fields) = retry {
            value
                .as_object_mut()
                .expect("operation object")
                .extend(fields);
        }
        if let Some(on_error) = on_error {
            value["on_error"] = Value::String(on_error.to_string());
        }
        serde_json::from_value(value).expect("task operation")
    }

    fn scheduling_declaration(value: Value) -> SchedulingOutcomeDeclaration {
        serde_json::from_value(value).expect("scheduling outcome declaration")
    }

    #[test]
    fn scheduling_outcome_coverage_requires_only_reachable_terminal_conditions() {
        let operations = vec![operation(json!({}), None)];
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned(), "alternate".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/terminal".to_owned(),
                "neutral/alternate".to_owned(),
            ],
            &operations,
            &declaration,
        )
        .expect_err("every initial target is a reachable no-effect terminal");

        let incomplete = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "wrong-page",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["home"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &operations,
            &incomplete,
        )
        .expect_err("reachable effect terminal must be covered");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );

        let complete = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminals",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal", "alternate"]
                }
            ]
        }));
        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned(), "alternate".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/terminal".to_owned(),
                "neutral/alternate".to_owned(),
            ],
            &operations,
            &complete,
        )
        .expect("unreachable designated-effect alternate terminal is not required");
    }

    #[test]
    fn scheduling_outcome_coverage_accepts_unique_effect_and_no_effect_paths() {
        let designated = operation(json!({}), None);
        let ordinary: TaskOperation = serde_json::from_value(json!({
            "id": "ordinary_terminal",
            "from": "alternate",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("ordinary operation");
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/alternate".to_owned(),
                "neutral/terminal".to_owned(),
            ],
            &[designated, ordinary],
            &declaration,
        )
        .expect("each mechanically reachable terminal condition has one mapping");
    }

    #[test]
    fn scheduling_outcome_coverage_uses_formal_operation_precedence() {
        let ordinary: TaskOperation = serde_json::from_value(json!({
            "id": "ordinary_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("ordinary operation");
        let mut shadowed_designated = operation(json!({}), None);
        shadowed_designated.to = None;
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[ordinary, shadowed_designated],
            &declaration,
        )
        .expect("a later same-page operation is unreachable under first-specific selection");
    }

    #[test]
    fn scheduling_outcome_coverage_does_not_treat_any_as_an_observable_page() {
        let designated = operation(json!({}), None);
        let fallback: TaskOperation = serde_json::from_value(json!({
            "id": "unreachable_fallback",
            "from": "any",
            "to": null,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("fallback operation");
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[designated, fallback],
            &declaration,
        )
        .expect("the fallback is shadowed across the complete observable page domain");
    }

    #[test]
    fn scheduling_outcome_coverage_canonicalizes_destination_anchors() {
        let first: TaskOperation = serde_json::from_value(json!({
            "id": "open_home",
            "from": "neutral/start",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("first operation");
        let second: TaskOperation = serde_json::from_value(json!({
            "id": "open_terminal",
            "from": "neutral/home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("second operation");
        let complete = scheduling_declaration(json!({
            "designated_operation": "open_home",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["neutral/terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["neutral/terminal"]
                }
            ]
        }));
        let observable_pages = [
            "neutral/start".to_owned(),
            "neutral/home".to_owned(),
            "neutral/terminal".to_owned(),
        ];

        validate_scheduling_outcome_coverage(
            "neutral",
            &["neutral/terminal".to_owned()],
            &observable_pages,
            &[first, second],
            &complete,
        )
        .expect("short destinations resolve to the unique concrete detector page");

        let missing_reachable_terminal = scheduling_declaration(json!({
            "designated_operation": "open_home",
            "mappings": [
                {
                    "outcome_key": "wrong-effect-page",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["neutral/home"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["neutral/terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["neutral/terminal".to_owned()],
            &observable_pages,
            &[
                serde_json::from_value(json!({
                    "id": "open_home",
                    "from": "neutral/start",
                    "to": "home",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true
                }))
                .expect("first operation"),
                serde_json::from_value(json!({
                    "id": "open_terminal",
                    "from": "neutral/home",
                    "to": "terminal",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true
                }))
                .expect("second operation"),
            ],
            &missing_reachable_terminal,
        )
        .expect_err("the concrete intermediate page must expose the reachable terminal");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );
    }

    #[test]
    fn scheduling_outcome_coverage_rejects_unknown_and_repeated_effect_paths() {
        let mut unknown = operation(json!({}), None);
        unknown.to = None;
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[unknown],
            &declaration,
        )
        .expect_err("mapped operation without a finite postcondition must fail admission");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );

        let mut cycle = operation(json!({}), None);
        cycle.to = Some(PageDeclaration::Singleton("home".to_owned()));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[cycle],
            &declaration,
        )
        .expect_err("a reachable second designated effect must fail admission");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );
    }

    #[test]
    fn recognize_only_rejects_scheduling_outcome_at_admission() {
        let mut recognize_only = control();
        recognize_only.execution_mode = "recognize_only".to_owned();

        let error = validate_scheduling_outcome_execution_mode(&recognize_only)
            .expect_err("recognize-only has no finite declared terminal-page closure");

        assert_eq!(error.code(), "contained_task_outcome_declaration_invalid");
    }

    fn omitted_policy_task(with_destination: bool, with_error_page: bool) -> PreparedContainedTask {
        let control = control();
        let mut task_operation = operation(json!({}), None);
        if !with_destination {
            task_operation.to = None;
        }
        task_operation.unguarded_trusted_coordinate = false;
        task_operation.guard = Some(
            serde_json::from_value(json!({
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            }))
            .expect("operation guard"),
        );
        task_operation
            .validate(&control, TaskOperationDefaults::default())
            .expect("valid omitted-policy operation");
        let program = TaskProgram {
            schema_version: "0.6".to_string(),
            task_id: "task".to_string(),
            game: "neutral".to_string(),
            server_scope: vec!["test".to_string()],
            coordinate_space: control.resolution,
            timeout_ms: None,
            max_steps: None,
            target_page: Some(PageDeclaration::Singleton("terminal".to_string())),
            error_pages: if with_error_page {
                vec!["error".to_string()]
            } else {
                Vec::new()
            },
            scheduling_outcome: None,
            post_admission_ocr: None,
            stability_termination: None,
            recovery: None,
            defaults: TaskOperationDefaults::default(),
            operations: vec![task_operation],
        };
        let pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.3",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {
                    "type": "color",
                    "id": "page/home",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 0, 0]
                },
                {
                    "type": "color",
                    "id": "page/terminal",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [0, 0, 255]
                },
                {
                    "type": "color",
                    "id": "page/alternate",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 0, 255]
                },
                {
                    "type": "color",
                    "id": "page/error",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 255, 0]
                },
                {
                    "type": "color",
                    "id": "guard/ready",
                    "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                    "expected": [0, 255, 0]
                }
            ]
        }))
        .expect("recognition pack");
        let evaluator =
            RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator");
        let page_set: PageSet = serde_json::from_value(json!({
            "schema_version": "0.3",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]},
                {"id": "neutral/alternate", "required": ["page/alternate"]},
                {"id": "neutral/error", "required": ["page/error"]}
            ]
        }))
        .expect("page set");
        let detector = PageDetector::new(page_set).expect("page detector");
        detector
            .validate(&evaluator)
            .expect("page detector targets");
        PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            scheduling_outcome: None,
            post_admission_ocr: None,
            package_sha256: "fixture-sha256".to_string(),
            entry_count: 5,
            task_count: 1,
        }
    }

    fn page_frame(page: &str) -> Frame {
        let page_color = match page {
            "home" => [255, 0, 0],
            "terminal" => [0, 0, 255],
            "alternate" => [255, 0, 255],
            "error" => [255, 255, 0],
            _ => panic!("unknown fixture page"),
        };
        Frame::from_pixels(
            2,
            1,
            [page_color, [0, 255, 0]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
    }

    fn unrecognized_frame() -> Frame {
        Frame::from_pixels(
            2,
            1,
            [0, 0, 0, 0, 255, 0].to_vec(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("unrecognized fixture frame")
    }

    struct ScriptedRuntime {
        frames: VecDeque<Frame>,
        last_frame: Frame,
        captures: usize,
        inputs: usize,
        traces: Vec<ContainedTaskTrace>,
    }

    impl ScriptedRuntime {
        fn new(after_effect_page: &str) -> Self {
            Self::from_pages("home", after_effect_page)
        }

        fn from_pages(initial_page: &str, after_effect_page: &str) -> Self {
            let last_frame = page_frame(after_effect_page);
            Self {
                frames: [page_frame(initial_page), last_frame.clone()].into(),
                last_frame,
                captures: 0,
                inputs: 0,
                traces: Vec::new(),
            }
        }
    }

    impl ContainedTaskRuntime for ScriptedRuntime {
        type Error = &'static str;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            self.captures += 1;
            Ok(match self.frames.pop_front() {
                Some(frame) => frame,
                None => self.last_frame.clone(),
            })
        }

        fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
            self.inputs += 1;
            Ok(())
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            self.traces.push(trace);
            Ok(())
        }
    }

    #[test]
    fn independent_max_steps_stops_the_no_end_path_at_61_instead_of_default_100() {
        let run_to_scheduler =
            |task: &PreparedContainedTask, runtime: &mut ScriptedRuntime| match task
                .run(runtime)
                .expect_err("no-end path must pause")
            {
                ContainedTaskRunError::Task(error) => error,
                ContainedTaskRunError::Boundary(error) => {
                    panic!("unexpected fixture boundary error: {error}")
                }
            };

        let mut bounded = omitted_policy_task(false, false);
        bounded.program.schema_version = "0.7".to_string();
        bounded.program.max_steps = Some(61);
        bounded.control.max_steps = Some(61);
        bounded
            .program
            .validate_task_max_steps(&bounded.control)
            .expect("exact source/control max-steps binding");
        let mut bounded_runtime = ScriptedRuntime::new("home");
        assert_eq!(
            run_to_scheduler(&bounded, &mut bounded_runtime).code(),
            "contained_task_requires_scheduler"
        );
        assert_eq!(bounded_runtime.inputs, 61);
        assert_eq!(
            bounded_runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::StepFinished { .. }))
                .count(),
            61
        );

        let default = omitted_policy_task(false, false);
        let mut default_runtime = ScriptedRuntime::new("home");
        assert_eq!(
            run_to_scheduler(&default, &mut default_runtime).code(),
            "contained_task_requires_scheduler"
        );
        assert_eq!(default_runtime.inputs, DEFAULT_MAX_STEPS as usize);
    }

    struct TimingRuntime {
        inner: ScriptedRuntime,
        captures_at: Vec<Instant>,
        effects_completed_at: Vec<Instant>,
    }

    impl TimingRuntime {
        fn new(inner: ScriptedRuntime) -> Self {
            Self {
                inner,
                captures_at: Vec::new(),
                effects_completed_at: Vec::new(),
            }
        }
    }

    impl ContainedTaskRuntime for TimingRuntime {
        type Error = &'static str;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            self.captures_at.push(Instant::now());
            self.inner.capture()
        }

        fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
            self.inner.input(action)
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            let effect_completed = matches!(&trace, ContainedTaskTrace::EffectCompleted { .. });
            self.inner.record(trace)?;
            if effect_completed {
                self.effects_completed_at.push(Instant::now());
            }
            Ok(())
        }
    }

    fn run_omitted_policy(
        with_destination: bool,
        with_error_page: bool,
        after_effect_page: &str,
    ) -> (
        Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
        ScriptedRuntime,
    ) {
        let task = omitted_policy_task(with_destination, with_error_page);
        let mut runtime = ScriptedRuntime::new(after_effect_page);
        let result = task.run(&mut runtime);
        (result, runtime)
    }

    fn assert_single_effect(runtime: &ScriptedRuntime) {
        assert_eq!(runtime.inputs, 1);
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::EffectIntent { .. }))
                .count(),
            1
        );
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. }))
                .count(),
            1
        );
    }

    fn assert_closed_effect_attempts(runtime: &ScriptedRuntime, attempts: usize) {
        let sequence = runtime
            .traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted { .. } => Some("step_started"),
                ContainedTaskTrace::EffectIntent { .. } => Some("effect_intent"),
                ContainedTaskTrace::EffectCompleted { .. } => Some("effect_completed"),
                ContainedTaskTrace::StepFinished { .. } => Some("step_finished"),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            sequence,
            [
                "step_started",
                "effect_intent",
                "effect_completed",
                "step_finished",
            ]
            .repeat(attempts)
        );
    }

    fn assert_page_confirmation_failed(
        result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
        after_page: &str,
    ) {
        let error = match result.expect_err("destination confirmation must fail") {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };
        assert_eq!(error.code(), "page_confirmation_failed");
        assert!(
            error
                .detail()
                .is_some_and(|detail| detail.contains(&format!("after_page=neutral/{after_page}")))
        );
    }

    #[test]
    fn omitted_policy_destination_reached_succeeds_after_fresh_observation() {
        let (result, runtime) = run_omitted_policy(true, false, "terminal");
        let outcome = result.expect("reached destination");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn post_delay_is_bounded_and_parse_overflow_fails_closed() {
        let task_control = control();
        let omitted = operation(json!({}), None);
        assert_eq!(omitted.post_delay_ms, None);
        omitted
            .validate(&task_control, TaskOperationDefaults::default())
            .expect("omitted post delay preserves existing admission");

        for invalid in [0, MAX_CAPTURE_INTERVAL_MS + 1, u64::MAX] {
            let mut operation = operation(json!({}), None);
            operation.post_delay_ms = Some(invalid);
            assert_eq!(
                operation
                    .validate(&task_control, TaskOperationDefaults::default())
                    .expect_err("invalid post delay must fail admission")
                    .code(),
                "contained_task_operation_invalid"
            );
        }

        assert!(
            serde_json::from_str::<TaskOperation>(
                r#"{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0},
                    "post_delay_ms":18446744073709551616,
                    "unguarded_trusted_coordinate":true
                }"#,
            )
            .is_err(),
            "a value outside u64 must fail during admission parsing"
        );
    }

    #[test]
    fn post_delay_precedes_each_same_page_postcondition_capture() {
        const DELAY_MS: u64 = 20;
        let mut task = stability_task(1, 2);
        task.control.timeout_ms = Some(500);
        task.program.operations[0].to = Some(PageDeclaration::Singleton("home".to_string()));
        task.program.operations[0].post_delay_ms = Some(DELAY_MS);
        let inner = stability_runtime(vec![
            stability_frame([10, 10, 10]),
            stability_frame([10, 10, 10]),
            stability_frame([10, 10, 10]),
        ]);
        let mut runtime = TimingRuntime::new(inner);

        let outcome = task.run(&mut runtime).expect("same-page stability success");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(runtime.inner.inputs, 2);
        assert_eq!(runtime.effects_completed_at.len(), 2);
        assert_eq!(runtime.captures_at.len(), 3);
        for (effect_completed_at, capture_at) in runtime
            .effects_completed_at
            .iter()
            .zip(runtime.captures_at.iter().skip(1))
        {
            assert!(
                capture_at.duration_since(*effect_completed_at) >= Duration::from_millis(DELAY_MS),
                "same-page capture occurred before its post-input delay"
            );
        }
    }

    #[test]
    fn post_delay_applies_once_before_a_polling_postcondition() {
        const DELAY_MS: u64 = 200;
        let mut task = omitted_policy_task(true, false);
        task.control.timeout_ms = Some(350);
        task.control.step_timeout_ms = Some(40);
        task.program.operations[0].post_delay_ms = Some(DELAY_MS);
        let terminal = page_frame("terminal");
        let inner = ScriptedRuntime {
            frames: [page_frame("home"), unrecognized_frame(), terminal.clone()].into(),
            last_frame: terminal,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        };
        let mut runtime = TimingRuntime::new(inner);

        let outcome = task
            .run(&mut runtime)
            .expect("one post-input delay must leave time for bounded polling");

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_eq!(runtime.inner.inputs, 1);
        assert_eq!(runtime.effects_completed_at.len(), 1);
        assert_eq!(runtime.captures_at.len(), 3);
        assert!(
            runtime.captures_at[1].duration_since(runtime.effects_completed_at[0])
                >= Duration::from_millis(DELAY_MS)
        );
    }

    #[test]
    fn post_delay_timeout_and_failed_input_stop_before_post_capture() {
        let mut timed_out = omitted_policy_task(true, false);
        timed_out.control.timeout_ms = Some(20);
        timed_out.program.operations[0].post_delay_ms = Some(50);
        let mut timeout_runtime = ScriptedRuntime::new("terminal");
        let timeout = match timed_out
            .run(&mut timeout_runtime)
            .expect_err("insufficient delay budget must fail")
        {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };
        assert_eq!(timeout.code(), "contained_task_timeout");
        assert_eq!(timeout_runtime.inputs, 1);
        assert_eq!(timeout_runtime.captures, 1);
        assert_eq!(
            timeout_runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. }))
                .count(),
            1
        );

        struct FailingInputRuntime {
            inner: ScriptedRuntime,
            input_attempts: usize,
        }

        impl ContainedTaskRuntime for FailingInputRuntime {
            type Error = &'static str;

            fn capture(&mut self) -> Result<Frame, Self::Error> {
                self.inner.capture()
            }

            fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
                self.input_attempts += 1;
                Err("injected input failure")
            }

            fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
                self.inner.record(trace)
            }
        }

        let mut failed = omitted_policy_task(true, false);
        failed.program.operations[0].post_delay_ms = Some(50);
        let mut failed_runtime = FailingInputRuntime {
            inner: ScriptedRuntime::new("terminal"),
            input_attempts: 0,
        };
        assert!(matches!(
            failed.run(&mut failed_runtime),
            Err(ContainedTaskRunError::Boundary("injected input failure"))
        ));
        assert_eq!(failed_runtime.input_attempts, 1);
        assert_eq!(failed_runtime.inner.captures, 1);
        assert!(
            !failed_runtime
                .inner
                .traces
                .iter()
                .any(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. }))
        );
    }

    #[test]
    fn destination_without_expect_after_uses_the_configured_bounded_wait() {
        let mut task = omitted_policy_task(true, false);
        task.control.step_timeout_ms = Some(50);
        assert!(task.program.operations[0].expect_after.is_none());
        let terminal = page_frame("terminal");
        let mut runtime = ScriptedRuntime {
            frames: [page_frame("home"), page_frame("home"), terminal.clone()].into(),
            last_frame: terminal,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        };

        let outcome = task
            .run(&mut runtime)
            .expect("bounded wait must observe the later destination");

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_eq!(runtime.captures, 3);
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_unchanged_page_fails_once_without_retry() {
        let (result, runtime) = run_omitted_policy(true, false, "home");

        assert_page_confirmation_failed(result, "home");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_declared_error_page_fails_once_without_recovery() {
        let (result, runtime) = run_omitted_policy(true, true, "error");

        assert_page_confirmation_failed(result, "error");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_without_destination_preserves_direct_success() {
        let (result, runtime) = run_omitted_policy(false, false, "terminal");
        let outcome = result.expect("operation without destination");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn destination_set_accepts_each_declared_fresh_page() {
        for destination in ["terminal", "alternate"] {
            let mut task = omitted_policy_task(true, false);
            task.program.operations[0].to = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            task.program.target_page = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            let mut runtime = ScriptedRuntime::new(destination);
            let outcome = task.run(&mut runtime).expect("declared destination");

            assert_eq!(
                outcome.final_page.as_deref(),
                Some(format!("neutral/{destination}").as_str())
            );
            assert_single_effect(&runtime);
        }
    }

    #[test]
    fn expect_after_is_the_canonical_postcondition_when_to_is_null() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.to = None;
        operation.expect_after = Some(
            serde_json::from_value(json!({
                "page_id": ["terminal", "alternate"],
                "timeout_ms": 2,
                "interval_ms": 1
            }))
            .expect("expect_after"),
        );
        task.program.target_page = Some(PageDeclaration::Set(vec![
            "terminal".to_string(),
            "alternate".to_string(),
        ]));
        let mut runtime = ScriptedRuntime::new("alternate");
        let outcome = task.run(&mut runtime).expect("expect_after destination");

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/alternate"));
        assert_single_effect(&runtime);
    }

    #[test]
    fn every_declared_terminal_page_completes_through_run_state() {
        for terminal in ["terminal", "alternate"] {
            let mut task = omitted_policy_task(true, false);
            task.program.target_page = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            let mut runtime = ScriptedRuntime::from_pages(terminal, terminal);
            let outcome = task.run(&mut runtime).expect("terminal page");

            assert_eq!(
                outcome.final_page.as_deref(),
                Some(format!("neutral/{terminal}").as_str())
            );
            assert_eq!(runtime.inputs, 0);
            assert_eq!(runtime.captures, 1);
        }
    }

    #[test]
    fn page_set_declarations_fail_closed_at_admission() {
        for invalid in [
            PageDeclaration::Set(Vec::new()),
            PageDeclaration::Set(vec!["terminal".to_string(), "terminal".to_string()]),
            PageDeclaration::Set(vec!["".to_string()]),
        ] {
            assert_eq!(
                invalid.normalized().expect_err("invalid page set").code(),
                "contained_task_page_set_invalid"
            );
        }

        let task = omitted_policy_task(true, false);
        let missing = vec!["missing".to_string()];
        assert_eq!(
            validate_page_references(&task.control.game, &missing, &task.detector)
                .expect_err("missing page reference")
                .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn conflicting_destination_declarations_fail_admission() {
        let mut operation = operation(json!({}), None);
        operation.expect_after =
            Some(serde_json::from_value(json!({"page_id": "alternate"})).expect("expect_after"));
        assert_eq!(
            operation
                .destination_pages()
                .expect_err("conflicting destinations")
                .code(),
            "contained_task_destination_conflict"
        );
    }

    #[test]
    fn destination_error_overlap_fails_before_execution() {
        let task = omitted_policy_task(true, true);
        assert_eq!(
            validate_page_set_overlap(
                &task.control.game,
                &["error".to_string()],
                &task.program.error_pages,
                &task.detector,
            )
            .expect_err("destination/error overlap")
            .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn alias_overlap_within_destination_set_fails_admission() {
        let task = omitted_policy_task(true, false);
        assert_eq!(
            validate_page_references(
                &task.control.game,
                &["terminal".to_string(), "neutral/terminal".to_string()],
                &task.detector,
            )
            .expect_err("ambiguous aliases")
            .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn operation_without_explicit_retry_policy_preserves_non_retry_behavior() {
        assert!(
            operation(json!({}), None)
                .retry_policy(TaskOperationDefaults::default(), 100)
                .expect("absent retry policy")
                .is_none()
        );
    }

    #[test]
    fn explicit_retry_policy_uses_existing_bounded_decision_owner() {
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 6,
                "retry_interval_ms": 1
            }),
            None,
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("retry policy")
            .expect("explicit retry policy");
        for attempt in 1..=5 {
            assert_eq!(
                operation
                    .failure_decision(
                        &policy,
                        attempt,
                        "page_confirmation_failed",
                        Some("home".to_string()),
                        RunFailureStage::PostExecution {
                            hit_error_page: false,
                        },
                    )
                    .expect("retry decision"),
                RunOperationFailureDecision::Retry {
                    next_attempt: attempt + 1,
                    delay_ms: 1,
                }
            );
        }
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    6,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("final decision"),
            RunOperationFailureDecision::Fail(_)
        ));
    }

    #[test]
    fn every_failed_retry_attempt_finishes_before_the_next_attempt_starts() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.retryable = Some(true);
        operation.max_attempts = Some(6);
        operation.retry_interval_ms = Some(1);
        let mut runtime = ScriptedRuntime::new("home");

        let error = match task
            .run(&mut runtime)
            .expect_err("sixth failed attempt must stop")
        {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };

        assert_eq!(error.code(), "contained_task_requires_scheduler");
        assert_eq!(runtime.inputs, 6);
        assert_closed_effect_attempts(&runtime, 6);
    }

    #[test]
    fn unrecognized_fresh_retry_observation_closes_without_second_effect() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.retryable = Some(true);
        operation.max_attempts = Some(6);
        operation.retry_interval_ms = Some(1);
        let unknown = unrecognized_frame();
        let mut runtime = ScriptedRuntime {
            frames: [page_frame("home"), page_frame("home"), unknown.clone()].into(),
            last_frame: unknown,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        };

        let error = match task
            .run(&mut runtime)
            .expect_err("unrecognized fresh observation must stop")
        {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };

        assert_eq!(error.code(), "page_confirmation_failed");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn explicit_retry_policy_consumes_existing_task_defaults() {
        let operation = operation(json!({"retryable": true}), None);
        let policy = operation
            .retry_policy(
                TaskOperationDefaults {
                    max_attempts: Some(3),
                    retry_interval_ms: Some(1),
                },
                100,
            )
            .expect("retry policy from task defaults")
            .expect("explicit retry policy");

        assert!(policy.retryable());
        assert_eq!(policy.max_attempts(), 3);
        assert_eq!(policy.retry_interval_ms(), 1);
    }

    #[test]
    fn non_retryable_and_invalid_policies_fail_closed() {
        let non_retryable = operation(json!({"retryable": false}), None);
        let policy = non_retryable
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("non-retryable policy")
            .expect("explicit non-retryable policy");
        assert!(matches!(
            non_retryable
                .failure_decision(
                    &policy,
                    1,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("non-retryable decision"),
            RunOperationFailureDecision::Fail(_)
        ));

        for invalid in [
            json!({"retryable": true}),
            json!({"retryable": true, "max_attempts": 0, "retry_interval_ms": 1}),
            json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 101}),
            json!({"max_attempts": 2, "retry_interval_ms": 1}),
        ] {
            assert_eq!(
                operation(invalid, None)
                    .retry_policy(TaskOperationDefaults::default(), 100)
                    .expect_err("invalid retry policy")
                    .code(),
                "contained_task_operation_invalid"
            );
        }
    }

    #[test]
    fn explicit_non_retryable_operation_without_destination_remains_valid() {
        let operation: TaskOperation = serde_json::from_value(json!({
            "id": "record_observation",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }))
        .expect("non-retryable operation");

        operation
            .validate(&control(), TaskOperationDefaults::default())
            .expect("explicitly non-retryable operation without to");
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("non-retryable policy")
            .expect("explicit policy");
        assert!(!policy.retryable());
        assert_eq!(policy.max_attempts(), 1);
    }

    #[test]
    fn xorshift64_uniform_rect_v1_matches_fixed_vectors() {
        for (seed, expected) in [
            (
                0,
                [
                    0xdc1b_77ae_0bf3_4dad,
                    0x64f0_eeb9_026e_6076,
                    0x7b07_ce91_e590_6136,
                    0x305f_050c_368d_cc74,
                ],
            ),
            (
                1,
                [
                    0x0000_0000_4082_2041,
                    0x1000_4106_0c01_1441,
                    0x9b1e_842f_6e86_2629,
                    0xf554_f503_555d_8025,
                ],
            ),
            (
                77,
                [
                    0x0000_0013_6613_b30d,
                    0xd013_0cad_5c04_f72b,
                    0xe422_98ab_316e_5405,
                    0x94ae_7d49_a5c3_29ed,
                ],
            ),
        ] {
            let mut state = normalized_xorshift64_state(seed);
            let actual = std::array::from_fn(|_| next_xorshift64(&mut state));
            assert_eq!(actual, expected, "seed {seed}");
        }
    }

    #[test]
    fn region_bearing_clicks_sample_inside_their_half_open_regions() {
        let resolution = Resolution {
            width: 100,
            height: 100,
        };
        for kind in ["rect", "specific_rect"] {
            let click: TaskClick = serde_json::from_value(json!({
                "kind": kind,
                "x": 10,
                "y": 20,
                "width": 8,
                "height": 6
            }))
            .expect("region click");
            let (action, sampling) = click
                .input_action(&resolution, None, Some(77))
                .expect("sampled region click");
            let InputAction::Tap { x, y } = action else {
                panic!("expected sampled tap")
            };
            assert!((10..18).contains(&x));
            assert!((20..26).contains(&y));
            let sampling = sampling.expect("sampling evidence");
            assert_eq!(sampling.action_seed(), 77);
            assert_eq!(sampling.source_regions().len(), 1);
        }

        let target = TargetEvaluation {
            id: "button".to_string(),
            kind: TargetKind::Template,
            passed: true,
            template: Some(actingcommand_recognition_pack::TemplateEvaluation {
                x: 30,
                y: 40,
                width: 10,
                height: 8,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.9,
            }),
            color: None,
            ocr: None,
            nn: None,
            message: "matched".to_string(),
        };
        for click in [
            serde_json::from_value::<TaskClick>(json!({
                "kind": "target",
                "target_id": "button"
            }))
            .expect("target click"),
            serde_json::from_value::<TaskClick>(json!({
                "kind": "offset",
                "target_id": "button",
                "offset": {"x": 2, "y": 1, "width": 4, "height": 3}
            }))
            .expect("offset click"),
        ] {
            let (action, sampling) = click
                .input_action(&resolution, Some(&target), Some(1))
                .expect("sampled target click");
            let InputAction::Tap { x, y } = action else {
                panic!("expected sampled target tap")
            };
            let region = sampling.expect("sampling evidence").source_regions()[0];
            assert!((region.x()..region.x() + region.width()).contains(&x));
            assert!((region.y()..region.y() + region.height()).contains(&y));
        }
    }

    #[test]
    fn drag_endpoints_use_one_seed_and_independent_region_samples() {
        let resolution = Resolution {
            width: 100,
            height: 100,
        };
        let click: TaskClick = serde_json::from_value(json!({
            "kind": "drag",
            "from_rect": {"x": 1, "y": 2, "width": 10, "height": 11},
            "to_rect": {"x": 50, "y": 60, "width": 12, "height": 13},
            "duration_ms": 250
        }))
        .expect("drag click");
        let (action, sampling) = click
            .input_action(&resolution, None, Some(1))
            .expect("sampled drag");
        let InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } = action
        else {
            panic!("expected sampled swipe")
        };
        assert!((1..11).contains(&x1));
        assert!((2..13).contains(&y1));
        assert!((50..62).contains(&x2));
        assert!((60..73).contains(&y2));
        assert_eq!(duration_ms, 250);
        let sampling = sampling.expect("sampling evidence");
        assert_eq!(sampling.source_regions().len(), 2);
        assert_ne!((x1 - 1, y1 - 2), (x2 - 50, y2 - 60));
    }

    #[test]
    fn non_degenerate_regions_vary_while_explicit_semantics_stay_exact() {
        let resolution = Resolution {
            width: 100,
            height: 100,
        };
        let rect: TaskClick = serde_json::from_value(json!({
            "kind": "rect",
            "x": 10,
            "y": 20,
            "width": 8,
            "height": 6
        }))
        .expect("rect click");
        let mut points = BTreeSet::new();
        for seed in 1..=16 {
            let (InputAction::Tap { x, y }, Some(_)) = rect
                .input_action(&resolution, None, Some(seed))
                .expect("sampled point")
            else {
                panic!("expected sampled tap")
            };
            assert!((10..18).contains(&x));
            assert!((20..26).contains(&y));
            points.insert((x, y));
        }
        assert!(points.len() > 1);

        let point: TaskClick = serde_json::from_value(json!({
            "kind": "point",
            "x": 7,
            "y": 9
        }))
        .expect("point click");
        assert_eq!(
            point
                .input_action(&resolution, None, Some(77))
                .expect("explicit point"),
            (InputAction::Tap { x: 7, y: 9 }, None)
        );
        let long_tap: TaskClick = serde_json::from_value(json!({
            "kind": "long_tap",
            "x": 8,
            "y": 10,
            "duration_ms": 500
        }))
        .expect("long tap");
        assert_eq!(
            long_tap
                .input_action(&resolution, None, Some(78))
                .expect("explicit long tap"),
            (
                InputAction::LongTap {
                    x: 8,
                    y: 10,
                    duration_ms: 500
                },
                None
            )
        );

        let target = TargetEvaluation {
            id: "button".to_string(),
            kind: TargetKind::Template,
            passed: true,
            template: Some(actingcommand_recognition_pack::TemplateEvaluation {
                x: 30,
                y: 40,
                width: 10,
                height: 8,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.9,
            }),
            color: None,
            ocr: None,
            nn: None,
            message: "matched".to_string(),
        };
        let target_center: TaskClick = serde_json::from_value(json!({
            "kind": "target_center",
            "target_id": "button"
        }))
        .expect("target center");
        assert_eq!(
            target_center
                .input_action(&resolution, Some(&target), Some(79))
                .expect("explicit target center"),
            (InputAction::Tap { x: 35, y: 44 }, None)
        );
    }

    #[test]
    fn effect_intent_persistence_failure_stops_before_sampled_input() {
        struct FailingRuntime {
            inner: ScriptedRuntime,
        }

        impl ContainedTaskRuntime for FailingRuntime {
            type Error = &'static str;

            fn capture(&mut self) -> Result<Frame, Self::Error> {
                self.inner.capture()
            }

            fn action_seed(
                &mut self,
                _step_index: u32,
                _operation_label: &str,
            ) -> Result<Option<u64>, Self::Error> {
                Ok(Some(77))
            }

            fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
                self.inner.input(action)
            }

            fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
                if matches!(trace, ContainedTaskTrace::EffectIntent { .. }) {
                    Err("injected effect-intent persistence failure")
                } else {
                    self.inner.record(trace)
                }
            }
        }

        let mut task = omitted_policy_task(false, false);
        task.program.operations[0].click = serde_json::from_value(json!({
            "kind": "rect",
            "x": 0,
            "y": 0,
            "width": 2,
            "height": 1
        }))
        .expect("region click");
        let mut runtime = FailingRuntime {
            inner: ScriptedRuntime::new("terminal"),
        };
        let error = task.run(&mut runtime).expect_err("persistence must fail");
        assert!(matches!(
            error,
            ContainedTaskRunError::Boundary("injected effect-intent persistence failure")
        ));
        assert_eq!(runtime.inner.inputs, 0);
    }

    #[test]
    fn invalid_sampling_region_fails_before_an_action_exists() {
        let error = sampled_tap(
            ClickRect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Some(77),
        )
        .expect_err("invalid sampling region");
        assert_eq!(error.code(), "contained_task_operation_invalid");
    }

    #[test]
    fn declared_error_page_requests_recovery_without_ordinary_retry() {
        let program: TaskProgram = serde_json::from_value(json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "coordinate_space": {"width": 2, "height": 1},
            "error_pages": ["error"],
            "operations": [{
                "id": "open_terminal",
                "from": "home",
                "to": "terminal",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true
            }]
        }))
        .expect("task program");
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 6,
                "retry_interval_ms": 1
            }),
            Some("return_home"),
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("retry policy")
            .expect("explicit retry policy");
        let hit_error_page = program.is_error_page(&control(), "neutral/error");

        assert!(hit_error_page);
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    1,
                    "page_confirmation_failed",
                    Some("neutral/error".to_string()),
                    RunFailureStage::PostExecution { hit_error_page },
                )
                .expect("error-page decision"),
            RunOperationFailureDecision::RequestRecovery(trigger)
                if trigger.operation_id == "open_terminal"
                    && trigger.attempts == 1
                    && trigger.after_page.as_deref() == Some("neutral/error")
                    && trigger.recovery_task_id == "return_home"
        ));
    }

    #[test]
    fn final_retry_decision_preserves_existing_recovery_path() {
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 2,
                "retry_interval_ms": 1
            }),
            Some("return_home"),
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("recovery policy")
            .expect("explicit recovery policy");
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    2,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("recovery decision"),
            RunOperationFailureDecision::RequestRecovery(trigger)
                if trigger.operation_id == "open_terminal"
                    && trigger.attempts == 2
                    && trigger.recovery_task_id == "return_home"
        ));
    }

    fn stability_declaration(
        region: StabilityRegion,
        consecutive_unchanged_threshold: u32,
        max_steps: u32,
    ) -> StabilityTerminationDeclaration {
        serde_json::from_value(json!({
            "region": {
                "x": region.x,
                "y": region.y,
                "width": region.width,
                "height": region.height
            },
            "comparison": {
                "mode": "exact_pixels_v1",
                "parameters": {}
            },
            "consecutive_unchanged_threshold": consecutive_unchanged_threshold,
            "max_steps": max_steps
        }))
        .expect("stability termination declaration")
    }

    fn stability_task(
        consecutive_unchanged_threshold: u32,
        max_steps: u32,
    ) -> PreparedContainedTask {
        let mut task = omitted_policy_task(false, false);
        let declaration = stability_declaration(
            StabilityRegion {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            consecutive_unchanged_threshold,
            max_steps,
        );
        task.control.execution_mode = "in_page_guard".to_string();
        task.control.max_steps = Some(max_steps);
        task.control.stability_termination = Some(declaration.clone());
        task.program.target_page = None;
        task.program.scheduling_outcome = None;
        task.program.stability_termination = Some(declaration);
        task.program.operations[0].to = None;
        task.program.operations[0].guard = None;
        task.program.operations[0].unguarded_trusted_coordinate = true;
        task
    }

    fn stability_frame(sample: [u8; 3]) -> Frame {
        Frame::from_pixels(
            2,
            1,
            [[255, 0, 0], sample].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("stability fixture frame")
    }

    fn stability_runtime(frames: Vec<Frame>) -> ScriptedRuntime {
        let last_frame = frames.last().expect("at least one frame").clone();
        ScriptedRuntime {
            frames: frames.into(),
            last_frame,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        }
    }

    #[test]
    fn stability_declaration_admission_is_exact_and_fail_closed() {
        let mut control_json = json!({
            "schema_version": CONTROL_SCHEMA,
            "package_id": "neutral.test.task",
            "execution_mode": "in_page_guard",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task"
        });
        assert!(
            serde_json::from_value::<TaskControl>(control_json.clone()).is_ok(),
            "an omitted control declaration must preserve legacy absence"
        );
        control_json["stability_termination"] = Value::Null;
        assert!(
            serde_json::from_value::<TaskControl>(control_json).is_err(),
            "explicit null control declaration must fail closed"
        );

        let mut program_json = json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "coordinate_space": {"width": 2, "height": 1},
            "operations": []
        });
        assert!(
            serde_json::from_value::<TaskProgram>(program_json.clone()).is_ok(),
            "an omitted task declaration must preserve legacy absence"
        );
        program_json["stability_termination"] = Value::Null;
        assert!(
            serde_json::from_value::<TaskProgram>(program_json).is_err(),
            "explicit null task declaration must fail closed"
        );

        let declaration = stability_declaration(
            StabilityRegion {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            2,
            4,
        );
        let mut active = control();
        active.execution_mode = "in_page_guard".to_string();
        active.max_steps = Some(4);
        active.stability_termination = Some(declaration.clone());
        active.validate().expect("valid stability control");

        let mut task = stability_task(2, 4);
        validate_stability_contract(&task.control, &task.program)
            .expect("matching control and task declarations");

        task.program
            .stability_termination
            .as_mut()
            .expect("task declaration")
            .max_steps = 5;
        assert_eq!(
            validate_stability_contract(&task.control, &task.program)
                .expect_err("task declaration must exactly match control")
                .code(),
            "contained_task_program_invalid"
        );

        let mut missing = control();
        missing.execution_mode = "in_page_guard".to_string();
        missing.max_steps = Some(4);
        missing
            .validate()
            .expect("feature absence preserves legacy in-page guard behavior");
        let mut legacy_program = stability_task(2, 4);
        legacy_program.control.stability_termination = None;
        legacy_program.program.stability_termination = None;
        legacy_program.program.target_page = Some(PageDeclaration::Singleton("home".to_string()));
        validate_stability_contract(&legacy_program.control, &legacy_program.program)
            .expect("matching absence preserves the legacy task contract");

        for mode in ["recognize_only", "navigable_route"] {
            let mut rejected = active.clone();
            rejected.execution_mode = mode.to_string();
            assert_eq!(
                rejected
                    .validate()
                    .expect_err("other modes reject stability termination")
                    .code(),
                "contained_task_control_invalid"
            );
        }

        for (threshold, nested_max, root_max) in
            [(0, 4, 4), (4, 4, 4), (2, 1_001, 1_001), (2, 4, 5)]
        {
            let mut rejected = active.clone();
            rejected.max_steps = Some(root_max);
            let stability = rejected
                .stability_termination
                .as_mut()
                .expect("stability declaration");
            stability.consecutive_unchanged_threshold = threshold;
            stability.max_steps = nested_max;
            assert_eq!(
                rejected
                    .validate()
                    .expect_err("invalid threshold or max-step relation")
                    .code(),
                "contained_task_control_invalid"
            );
        }

        for region in [
            StabilityRegion {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            StabilityRegion {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
            StabilityRegion {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            let mut rejected = active.clone();
            rejected
                .stability_termination
                .as_mut()
                .expect("stability declaration")
                .region = region;
            assert_eq!(
                rejected
                    .validate()
                    .expect_err("invalid or unbounded crop")
                    .code(),
                "contained_task_control_invalid"
            );
        }

        let mut byte_layout_overflow = active.clone();
        byte_layout_overflow.resolution = Resolution {
            width: u32::MAX,
            height: u32::MAX,
        };
        let overflow_declaration = byte_layout_overflow
            .stability_termination
            .as_mut()
            .expect("stability declaration");
        overflow_declaration.region = StabilityRegion {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
        };
        assert_eq!(
            byte_layout_overflow
                .validate()
                .expect_err("checked four-byte frame layout must fit the platform")
                .code(),
            "contained_task_control_invalid"
        );

        let mut target_conflict = stability_task(2, 4);
        target_conflict.program.target_page = Some(PageDeclaration::Singleton("home".to_string()));
        assert_eq!(
            validate_stability_contract(&target_conflict.control, &target_conflict.program)
                .expect_err("stability rejects target-page terminal ownership")
                .code(),
            "contained_task_program_invalid"
        );
        let mut outcome_conflict = stability_task(2, 4);
        outcome_conflict.program.scheduling_outcome = Some(scheduling_declaration(json!({
            "mappings": [{
                "outcome_key": "comparison_recorded",
                "effect": "no_designated_effect",
                "terminal_pages": ["home"]
            }]
        })));
        assert_eq!(
            validate_stability_contract(&outcome_conflict.control, &outcome_conflict.program)
                .expect_err("stability rejects scheduling terminal ownership")
                .code(),
            "contained_task_program_invalid"
        );

        let post_admission_ocr = |outcome_key: &str| {
            serde_json::from_value(serde_json::json!({
                "page_id": "home",
                "target_id": "fixture/ocr",
                "truth_set": {"path": "truth.json", "sha256": "c".repeat(64)},
                "normalization": "trim_lowercase_v1",
                "comparison": "exact_set_v1",
                "limits": {
                    "max_frames": 2,
                    "max_items": 16,
                    "max_string_bytes": 64,
                    "max_total_bytes": 4096,
                    "max_truth_entries": 16
                },
                "outcome_key": outcome_key
            }))
            .expect("post-admission OCR declaration")
        };
        let scheduling_outcome = |outcome_keys: &[&str]| {
            scheduling_declaration(serde_json::json!({
                "mappings": outcome_keys
                    .iter()
                    .map(|outcome_key| serde_json::json!({
                        "outcome_key": outcome_key,
                        "effect": "no_designated_effect",
                        "terminal_pages": ["home"]
                    }))
                    .collect::<Vec<_>>()
            }))
        };
        let mut owned_outcome = stability_task(2, 4);
        owned_outcome.program.schema_version = "0.7".to_string();
        owned_outcome.program.scheduling_outcome =
            Some(scheduling_outcome(&["comparison_recorded"]));
        owned_outcome.program.post_admission_ocr = Some(post_admission_ocr("comparison_recorded"));
        owned_outcome.program.operations[0].to =
            Some(PageDeclaration::Singleton("home".to_string()));
        validate_stability_contract(&owned_outcome.control, &owned_outcome.program)
            .expect("one matching OCR-selected outcome owns stability completion");

        for (case, mappings) in [
            ("missing", Vec::new()),
            (
                "duplicate",
                vec!["comparison_recorded", "comparison_recorded"],
            ),
            ("mismatch", vec!["other_result"]),
        ] {
            let mut rejected = stability_task(2, 4);
            rejected.program.schema_version = "0.7".to_string();
            rejected.program.scheduling_outcome = Some(scheduling_outcome(&mappings));
            rejected.program.post_admission_ocr = Some(post_admission_ocr("comparison_recorded"));
            rejected.program.operations[0].to =
                Some(PageDeclaration::Singleton("home".to_string()));
            assert_eq!(
                validate_stability_contract(&rejected.control, &rejected.program)
                    .expect_err("missing, duplicate, or mismatched ownership must fail closed")
                    .code(),
                "contained_task_program_invalid",
                "case={case}"
            );
        }

        assert!(
            serde_json::from_value::<StabilityTerminationDeclaration>(json!({
                "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                "comparison": {"mode": "exact_pixels_v1", "parameters": {"extra": true}},
                "consecutive_unchanged_threshold": 2,
                "max_steps": 4
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<StabilityTerminationDeclaration>(json!({
                "region": {"x": 1, "y": 0, "width": 1, "height": 1, "extra": true},
                "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
                "consecutive_unchanged_threshold": 2,
                "max_steps": 4
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<StabilityComparisonParameters>(json!({})).is_ok());
        for invalid_parameters in [json!(null), json!([]), json!({"extra": true})] {
            assert!(
                serde_json::from_value::<StabilityComparisonParameters>(invalid_parameters)
                    .is_err(),
                "exact_pixels_v1 parameters must be exactly an empty object"
            );
        }
    }

    #[test]
    fn exact_pixels_v1_compares_only_the_checked_declared_crop() {
        let declaration = stability_declaration(
            StabilityRegion {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            2,
            4,
        );
        let frame = |left: [u8; 3], center: [u8; 3], right: [u8; 3], format| {
            Frame::from_pixels(
                3,
                1,
                [left, center, right].concat(),
                format,
                CaptureBackendName::FixtureSimulation,
            )
            .expect("raw crop fixture")
        };
        let baseline = stability_sample(
            &frame([1, 2, 3], [10, 11, 12], [4, 5, 6], PixelFormat::Rgb8),
            &declaration,
        )
        .expect("baseline sample");
        let outside_only = stability_sample(
            &frame([8, 9, 10], [10, 11, 12], [11, 12, 13], PixelFormat::Rgb8),
            &declaration,
        )
        .expect("outside-only change sample");
        assert_eq!(
            compare_stability_samples(&baseline, &outside_only, &declaration.comparison)
                .expect("comparable crop"),
            StabilityComparisonResult::Unchanged
        );

        let inside = stability_sample(
            &frame([1, 2, 3], [10, 11, 13], [4, 5, 6], PixelFormat::Rgb8),
            &declaration,
        )
        .expect("inside change sample");
        assert_eq!(
            compare_stability_samples(&baseline, &inside, &declaration.comparison)
                .expect("comparable crop"),
            StabilityComparisonResult::Changed
        );

        let other_format_frame = Frame::from_pixels(
            3,
            1,
            [[1, 2, 3, 255], [10, 11, 12, 255], [4, 5, 6, 255]].concat(),
            PixelFormat::Rgba8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("other-format frame");
        let other_format =
            stability_sample(&other_format_frame, &declaration).expect("other-format sample");
        assert_eq!(
            compare_stability_samples(&baseline, &other_format, &declaration.comparison)
                .expect_err("pixel-format drift fails closed")
                .code(),
            "contained_task_stability_comparison_failed"
        );
    }

    #[test]
    fn stability_resets_then_increments_and_stops_exactly_at_threshold() {
        let task = stability_task(2, 6);
        let mut runtime = stability_runtime(vec![
            stability_frame([1, 1, 1]),
            stability_frame([10, 10, 10]),
            stability_frame([10, 10, 10]),
            stability_frame([20, 20, 20]),
            stability_frame([20, 20, 20]),
            stability_frame([20, 20, 20]),
        ]);

        let outcome = task.run(&mut runtime).expect("stability terminal success");
        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.executed_steps, 5);
        assert_eq!(
            runtime.inputs, 5,
            "no input occurs after stability terminal"
        );

        let comparisons = runtime
            .traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StabilityComparison {
                    step_index,
                    result,
                    prior_consecutive_unchanged,
                    new_consecutive_unchanged,
                    terminal_reason,
                    ..
                } => Some((
                    *step_index,
                    *result,
                    *prior_consecutive_unchanged,
                    *new_consecutive_unchanged,
                    *terminal_reason,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            comparisons,
            vec![
                (1, StabilityComparisonResult::Unchanged, 0, 1, None),
                (2, StabilityComparisonResult::Changed, 1, 0, None),
                (3, StabilityComparisonResult::Unchanged, 0, 1, None),
                (
                    4,
                    StabilityComparisonResult::Unchanged,
                    1,
                    2,
                    Some(StabilityTerminalReason::ConsecutiveUnchangedThresholdReached)
                ),
            ]
        );
        assert!(matches!(
            runtime.traces.iter().find(|trace| matches!(
                trace,
                ContainedTaskTrace::StabilityBaseline { step_index: 0, .. }
            )),
            Some(_)
        ));

        let terminal = runtime
            .traces
            .iter()
            .position(|trace| {
                matches!(
                    trace,
                    ContainedTaskTrace::StabilityTerminal {
                        step_index: 4,
                        reason: StabilityTerminalReason::ConsecutiveUnchangedThresholdReached,
                        ..
                    }
                )
            })
            .expect("typed threshold terminal");
        let final_step = runtime
            .traces
            .iter()
            .position(|trace| {
                matches!(
                    trace,
                    ContainedTaskTrace::StepFinished { step_index: 4, .. }
                )
            })
            .expect("final step closure");
        let finalizing = runtime
            .traces
            .iter()
            .position(|trace| {
                matches!(
                    trace,
                    ContainedTaskTrace::Finalizing {
                        outcome: TaskOutcome::Success
                    }
                )
            })
            .expect("existing success finalization");
        assert!(final_step < terminal && terminal < finalizing);
    }

    #[test]
    fn stability_max_steps_emits_typed_terminal_and_existing_scheduler_error() {
        let task = stability_task(2, 4);
        let mut runtime = stability_runtime(vec![
            stability_frame([1, 1, 1]),
            stability_frame([10, 10, 10]),
            stability_frame([20, 20, 20]),
            stability_frame([30, 30, 30]),
            stability_frame([40, 40, 40]),
        ]);

        let error = match task.run(&mut runtime).expect_err("hard max must stop") {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };
        assert_eq!(error.code(), "contained_task_requires_scheduler");
        assert_eq!(runtime.inputs, 4);
        assert!(matches!(
            runtime.traces.last(),
            Some(ContainedTaskTrace::StabilityTerminal {
                step_index: 3,
                reason: StabilityTerminalReason::MaxStepsReached,
                ..
            })
        ));
        assert!(
            !runtime
                .traces
                .iter()
                .any(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }))
        );
    }

    #[test]
    fn stability_trace_failure_stops_before_step_commit_or_later_input() {
        struct FailingComparisonRuntime {
            inner: ScriptedRuntime,
        }

        impl ContainedTaskRuntime for FailingComparisonRuntime {
            type Error = &'static str;

            fn capture(&mut self) -> Result<Frame, Self::Error> {
                self.inner.capture()
            }

            fn input(&mut self, action: InputAction) -> Result<(), Self::Error> {
                self.inner.input(action)
            }

            fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
                if matches!(trace, ContainedTaskTrace::StabilityComparison { .. }) {
                    Err("injected stability comparison persistence failure")
                } else {
                    self.inner.record(trace)
                }
            }
        }

        let task = stability_task(2, 5);
        let inner = stability_runtime(vec![
            stability_frame([1, 1, 1]),
            stability_frame([10, 10, 10]),
            stability_frame([10, 10, 10]),
            stability_frame([10, 10, 10]),
        ]);
        let mut runtime = FailingComparisonRuntime { inner };
        assert!(matches!(
            task.run(&mut runtime),
            Err(ContainedTaskRunError::Boundary(
                "injected stability comparison persistence failure"
            ))
        ));
        assert_eq!(runtime.inner.inputs, 2);
        assert_eq!(
            runtime
                .inner
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::StepFinished { .. }))
                .count(),
            1,
            "failed comparison evidence does not commit the second step"
        );
        assert!(
            !runtime
                .inner
                .traces
                .iter()
                .any(|trace| matches!(trace, ContainedTaskTrace::StabilityTerminal { .. }))
        );
    }
}
