// SPDX-License-Identifier: AGPL-3.0-only

//! Target predicates share the candidate aggregation owner and accept caller-owned frames.

use crate::*;
use actingcommand_contract::candidate_projection::CandidateFeature;
use actingcommand_contract::{TaskDiagnosticSampleData, TaskDiagnosticSamplingData};
use sha2::{Digest, Sha256};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConsensus {
    pub samples: Vec<CandidateSampleVariant>,
    pub k: u8,
    #[serde(default)]
    pub sample_interval_ms: u16,
}

pub type TargetSampleRecorder<'a> = dyn FnMut(
        &str,
        &RecognitionPackResult<TargetEvaluation>,
        Instant,
        Instant,
    ) -> RecognitionPackResult<()>
    + 'a;

impl TargetConsensus {
    fn aggregation(&self) -> CandidateConsensus {
        CandidateConsensus {
            samples: self.samples.clone(),
            aggregate: CandidateAggregation::KOfN { k: self.k },
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.aggregation().validate(CandidateFeatureValue::Passed)?;
        let frames = self.required_frames();
        if self.sample_interval_ms > 1000 || (frames > 1 && self.sample_interval_ms == 0) {
            return Err("target consensus interval must be 1..=1000 ms for multiple frames".into());
        }
        if (0..frames).any(|frame| {
            !self
                .samples
                .iter()
                .any(|sample| usize::from(sample.frame) == frame)
        }) {
            return Err("target consensus frame indices must be contiguous from zero".into());
        }
        if !self.samples.iter().any(|sample| {
            usize::from(sample.frame) == frames - 1 && sample.dx == 0 && sample.dy == 0
        }) {
            return Err(
                "target consensus requires an unperturbed sample on the current frame".into(),
            );
        }
        Ok(())
    }

    pub fn required_frames(&self) -> usize {
        self.samples
            .iter()
            .map(|sample| usize::from(sample.frame) + 1)
            .max()
            .unwrap_or(1)
    }
}

pub(crate) fn validate_target_consensus(pack: &RecognitionPack, errors: &mut Vec<String>) {
    let mut interval = None;
    for target in &pack.targets {
        if let RecognitionTarget::Ocr(target) = target
            && let PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                anchor_target_id,
                ..
            }) = &target.region
            && pack.target_consensus.contains_key(anchor_target_id)
        {
            errors.push(format!(
                "OCR target '{}': its per-frame template anchor cannot declare target consensus",
                target.id
            ));
        }
    }
    for (id, declaration) in &pack.target_consensus {
        if let Err(error) = declaration.validate() {
            errors.push(format!("target_consensus/{id}: {error}"));
        }
        if declaration.required_frames() > 1 {
            if interval.is_some_and(|interval| interval != declaration.sample_interval_ms) {
                errors.push(
                    "target consensus declarations in one pack must share the frame interval"
                        .into(),
                );
            }
            interval = Some(declaration.sample_interval_ms);
        }
        let Some(target) = pack.targets.iter().find(|target| target.id() == id) else {
            errors.push(format!("target_consensus/{id}: target is not declared"));
            continue;
        };
        validate_sample_variants(pack, target, &declaration.samples, errors);
    }
}

pub(crate) fn validate_sample_variants(
    pack: &RecognitionPack,
    target: &RecognitionTarget,
    samples: &[CandidateSampleVariant],
    errors: &mut Vec<String>,
) {
    let effective = samples
        .iter()
        .map(|sample| sample.effective(pack.defaults.match_metric))
        .collect::<Vec<_>>();
    if effective
        .iter()
        .enumerate()
        .any(|(index, sample)| effective[..index].contains(sample))
    {
        errors.push(format!(
            "target '{}': duplicate effective frame/region/parameter samples",
            target.id()
        ));
    }
    for sample in samples {
        match sample.target(target) {
            Err(error) => errors.push(error.to_string()),
            Ok(target) => {
                let region = match target {
                    RecognitionTarget::Template(target) => target.region,
                    RecognitionTarget::Ocr(target) => target.region,
                    RecognitionTarget::Color(target) => PackRegion::Rect(target.region),
                    RecognitionTarget::ColorDigest(target) => PackRegion::Rect(target.region),
                    _ => continue,
                };
                validate_region_within_coordinate_space(
                    &region,
                    pack.coordinate_space,
                    "consensus sample",
                    errors,
                );
            }
        }
    }
    if let RecognitionTarget::Ocr(target) = target
        && let PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
            anchor_target_id,
            ..
        }) = &target.region
        && pack.target_consensus.contains_key(anchor_target_id)
    {
        errors.push("sampled OCR uses a per-frame template anchor; the anchor cannot itself declare target consensus".into());
    }
}

impl RecognitionEvaluator {
    pub fn target_sample_frames(&self) -> usize {
        self.pack
            .target_consensus
            .values()
            .map(TargetConsensus::required_frames)
            .max()
            .unwrap_or(1)
    }

    pub fn target_sample_interval_ms(&self) -> u64 {
        self.pack
            .target_consensus
            .values()
            .map(|value| u64::from(value.sample_interval_ms))
            .max()
            .unwrap_or(0)
    }

    /// All declared sampled targets are admitted conservatively, including guards.
    pub fn target_sample_provider_and_wait_ms(&self) -> u64 {
        self.pack
            .target_consensus
            .iter()
            .map(|(id, value)| match self.target(id) {
                Ok(RecognitionTarget::Ocr(target)) => {
                    target.timeout_ms * value.samples.len() as u64
                }
                _ => 0,
            })
            .sum::<u64>()
            + self.target_sample_interval_ms()
                * self.target_sample_frames().saturating_sub(1) as u64
    }

    pub fn scene_context_with_samples<'a>(
        &'a self,
        scenes: &[&'a Scene],
    ) -> RecognitionPackResult<SceneEvaluation<'a>> {
        let Some(scene) = scenes.last() else {
            return Err(RecognitionPackError::fatal(
                "recognition samples unavailable: empty frame set",
            ));
        };
        if scenes.len() > 5
            || scenes.iter().enumerate().any(|(index, candidate)| {
                candidate.width() != scene.width()
                    || candidate.height() != scene.height()
                    || scenes[..index]
                        .iter()
                        .any(|prior| std::ptr::eq(*prior, *candidate))
            })
        {
            return Err(RecognitionPackError::fatal(
                "recognition sample frames must be distinct captures with unchanged geometry, at most five",
            ));
        }
        let mut context = self.scene_context(scene);
        context.sample_scenes = scenes.to_vec();
        Ok(context)
    }
}

impl<'a> SceneEvaluation<'a> {
    pub fn with_sample_deadline(mut self, deadline: Instant) -> Self {
        self.sample_deadline = Some(deadline);
        self
    }

    pub fn with_sample_recorder(mut self, recorder: &'a mut TargetSampleRecorder<'a>) -> Self {
        self.sample_recorder = Some(RefCell::new(recorder));
        self
    }

    pub(crate) fn evaluate_consensus(
        &self,
        id: &str,
        declaration: &TargetConsensus,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let count = declaration.required_frames();
        if self.sample_scenes.len() < count {
            return Err(RecognitionPackError::fatal(format!(
                "target '{id}' recognition samples unavailable: requires {count} frames, got {}",
                self.sample_scenes.len()
            )));
        }
        let scenes = &self.sample_scenes[self.sample_scenes.len() - count..];
        let mut values = Vec::with_capacity(declaration.samples.len());
        let mut evidence = Vec::with_capacity(declaration.samples.len());
        let mut current = None;
        let mut reports = Vec::new();
        let mut sample_evaluations = Vec::new();
        for (index, sample) in declaration.samples.iter().enumerate() {
            let remaining_provider_ms = match self.evaluator.target(id)? {
                RecognitionTarget::Ocr(target) => {
                    target.timeout_ms * (declaration.samples.len() - index) as u64
                }
                _ => 0,
            };
            if self.sample_deadline.is_some_and(|deadline| {
                Instant::now() >= deadline
                    || std::time::Duration::from_millis(remaining_provider_ms)
                        > deadline.saturating_duration_since(Instant::now())
            }) {
                return Err(RecognitionPackError::fatal(
                    "recognition sample budget insufficient before backend evaluation",
                ));
            }
            let scene = scenes[usize::from(sample.frame)];
            let started = Instant::now();
            let mut result = self
                .evaluator
                .scene_context(scene)
                .evaluate_candidate_sample(id, *sample);
            let ended = Instant::now();
            let data = TaskDiagnosticSampleData {
                frame_index: sample.frame,
                frame_rgb8_sha256: format!("{:x}", Sha256::digest(scene.rgb8_pixels())),
                dx: sample.dx,
                dy: sample.dy,
                template_metric: matches!(
                    self.evaluator.target(id)?,
                    RecognitionTarget::Template(_)
                )
                .then(|| {
                    sample
                        .template_metric
                        .unwrap_or(self.evaluator.pack.defaults.match_metric)
                })
                .map(|metric| {
                    match metric {
                        RecognitionMatchMetric::CcorrNormed => "ccorr_normed",
                        RecognitionMatchMetric::CcoeffNormed => "ccoeff_normed",
                    }
                    .to_owned()
                }),
                elapsed_us: u64::try_from(ended.duration_since(started).as_micros())
                    .unwrap_or(u64::MAX),
                passed: result.as_ref().ok().map(|value| value.passed),
            };
            if let Ok(value) = &mut result {
                value.sampling = Some(TaskDiagnosticSamplingData::Sample {
                    sample: data.clone(),
                });
                evidence.push(data);
            } else if let Err(error) = &mut result {
                error.sample = Some(Box::new(data));
            }
            if let Some(recorder) = &self.sample_recorder {
                (recorder.borrow_mut())(id, &result, started, ended)?;
            }
            let value = result?;
            if self.sample_recorder.is_none() {
                sample_evaluations.push(value.clone());
            }
            reports.extend_from_slice(value.ppocr_diagnostics());
            values.push(CandidateFeature::Boolean {
                value: value.passed,
                confidence: None,
            });
            // Geometry is supplied only by the unperturbed current frame. A caller cannot
            // click an old winning sample's match rectangle.
            if usize::from(sample.frame) == count - 1 && sample.dx == 0 && sample.dy == 0 {
                current = Some(value);
            }
        }
        let mut result = current.ok_or_else(|| {
            RecognitionPackError::fatal(
                "target consensus requires an unperturbed sample on the current frame",
            )
        })?;
        let CandidateFeature::Boolean { value: passed, .. } =
            declaration.aggregation().aggregate(&values)
        else {
            return Err(RecognitionPackError::fatal("target consensus unresolved"));
        };
        result.passed = passed;
        result.message = format!(
            "target consensus: k={} samples={} passed={passed}",
            declaration.k,
            values.len()
        );
        if let Some(ocr) = &mut result.ocr {
            ocr.ppocr_diagnostics = if self.sample_recorder.is_none() {
                reports
            } else {
                Vec::new()
            };
        }
        result.sampling = Some(TaskDiagnosticSamplingData::Consensus {
            k: declaration.k,
            samples: evidence,
        });
        result.sample_evaluations = sample_evaluations;
        Ok(result)
    }
}
