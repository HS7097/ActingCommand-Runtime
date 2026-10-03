// SPDX-License-Identifier: AGPL-3.0-only

//! One bounded aggregation owner for Runtime and Lab. No captures, waits or durable state.

use crate::{
    CandidateFeatureValue, PackRect, PackRegion, RecognitionEvaluator, RecognitionMatchMetric,
    RecognitionPackError, RecognitionPackResult, RecognitionTarget, SceneEvaluation,
    TargetEvaluation, TemplateRelativeRegion,
};
use actingcommand_contract::candidate_projection::{CandidateFeature, CandidateUnknownReason};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_CANDIDATE_SAMPLES: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateConsensus {
    pub samples: Vec<CandidateSampleVariant>,
    pub aggregate: CandidateAggregation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSampleVariant {
    #[serde(default)]
    pub frame: u8,
    #[serde(default)]
    pub dx: i16,
    #[serde(default)]
    pub dy: i16,
    /// Existing template preprocessing/matching parameter; other kinds reject it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_metric: Option<RecognitionMatchMetric>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateAggregation {
    KOfN {
        k: u8,
    },
    /// The lower middle observed value for even N, without inventing a fractional reading.
    Median,
    Majority {
        k: u8,
    },
    ClosestSingle {
        maximum_distance: u16,
    },
}

impl CandidateConsensus {
    pub fn validate(&self, value: CandidateFeatureValue) -> Result<(), String> {
        if !(1..=MAX_CANDIDATE_SAMPLES).contains(&self.samples.len())
            || self.samples.iter().any(|sample| {
                sample.frame >= 5 || sample.dx.unsigned_abs() > 8 || sample.dy.unsigned_abs() > 8
            })
        {
            return Err("consensus requires 1..=5 samples; frame 0..=4 and jitter -8..=8".into());
        }
        if self
            .samples
            .iter()
            .enumerate()
            .any(|(index, sample)| self.samples[..index].contains(sample))
        {
            return Err(
                "consensus samples must differ in frame, region or recognition parameter".into(),
            );
        }
        let valid = match (value, self.aggregate) {
            (
                CandidateFeatureValue::Passed
                | CandidateFeatureValue::MeasureMilli
                | CandidateFeatureValue::OcrInteger,
                CandidateAggregation::KOfN { k },
            )
            | (CandidateFeatureValue::Identity, CandidateAggregation::Majority { k }) => {
                k > 0 && usize::from(k) <= self.samples.len()
            }
            (
                CandidateFeatureValue::MeasureMilli | CandidateFeatureValue::OcrInteger,
                CandidateAggregation::Median,
            ) => true,
            (
                CandidateFeatureValue::Identity,
                CandidateAggregation::ClosestSingle { maximum_distance },
            ) => maximum_distance <= 1000,
            _ => false,
        };
        if !valid {
            return Err("aggregation is incompatible with feature type or sample count".into());
        }
        Ok(())
    }

    pub fn aggregate(&self, values: &[CandidateFeature]) -> CandidateFeature {
        let unknown = || CandidateFeature::Unknown {
            reason: CandidateUnknownReason::NoConsensus,
            source: None,
        };
        if values.len() != self.samples.len()
            || values.is_empty()
            || self
                .samples
                .iter()
                .enumerate()
                .any(|(index, sample)| self.samples[..index].contains(sample))
        {
            return unknown();
        }
        let confidence = values
            .iter()
            .map(|value| match value {
                CandidateFeature::Integer { confidence, .. }
                | CandidateFeature::Boolean { confidence, .. }
                | CandidateFeature::Identity { confidence, .. } => *confidence,
                CandidateFeature::Unknown { .. } => None,
            })
            .collect::<Option<Vec<_>>>()
            .and_then(|values| values.into_iter().min());
        match self.aggregate {
            CandidateAggregation::Median => {
                let Some(mut numbers) = values
                    .iter()
                    .map(|value| match value {
                        CandidateFeature::Integer { value, .. } => Some(*value),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    return unknown();
                };
                numbers.sort_unstable();
                CandidateFeature::Integer {
                    value: numbers[(numbers.len() - 1) / 2],
                    confidence,
                }
            }
            CandidateAggregation::KOfN { k } => {
                if values.iter().all(|value| {
                    matches!(
                        value,
                        CandidateFeature::Boolean { .. } | CandidateFeature::Unknown { .. }
                    )
                }) {
                    let yes = values
                        .iter()
                        .filter(|value| {
                            matches!(value, CandidateFeature::Boolean { value: true, .. })
                        })
                        .count();
                    let no = values
                        .iter()
                        .filter(|value| {
                            matches!(value, CandidateFeature::Boolean { value: false, .. })
                        })
                        .count();
                    if yes >= usize::from(k) {
                        CandidateFeature::Boolean {
                            value: true,
                            confidence,
                        }
                    } else if no > values.len() - usize::from(k) {
                        CandidateFeature::Boolean {
                            value: false,
                            confidence,
                        }
                    } else {
                        unknown()
                    }
                } else {
                    let mut counts = BTreeMap::new();
                    for value in values {
                        if let CandidateFeature::Integer { value, .. } = value {
                            *counts.entry(*value).or_insert(0_usize) += 1;
                        }
                    }
                    let winners = counts
                        .into_iter()
                        .filter(|(_, count)| *count >= usize::from(k))
                        .collect::<Vec<_>>();
                    match winners.as_slice() {
                        [(value, _)] => CandidateFeature::Integer {
                            value: *value,
                            confidence,
                        },
                        _ => unknown(),
                    }
                }
            }
            CandidateAggregation::Majority { .. } | CandidateAggregation::ClosestSingle { .. } => {
                // Votes group stable business identity plus variant; aliases/templates have
                // already been grouped by business ID inside each actual sample.
                let mut groups = BTreeMap::<(&str, &Option<String>), Vec<&CandidateFeature>>::new();
                for value in values {
                    if let CandidateFeature::Identity {
                        value: id, variant, ..
                    } = value
                    {
                        groups
                            .entry((id.as_str(), variant))
                            .or_default()
                            .push(value);
                    }
                }
                let minimum = |group: &[&CandidateFeature]| {
                    group
                        .iter()
                        .filter_map(|value| match value {
                            CandidateFeature::Identity { distance, .. } => Some(*distance),
                            _ => None,
                        })
                        .min()
                        .unwrap_or(1001)
                };
                let majority = matches!(self.aggregate, CandidateAggregation::Majority { .. });
                let mut groups = groups
                    .into_values()
                    .filter(|group| match self.aggregate {
                        CandidateAggregation::Majority { k } => group.len() >= usize::from(k),
                        CandidateAggregation::ClosestSingle { maximum_distance } => {
                            minimum(group) <= maximum_distance
                        }
                        _ => false,
                    })
                    .collect::<Vec<_>>();
                groups.sort_by_key(|group| {
                    (if majority { 5 - group.len() } else { 0 }, minimum(group))
                });
                let Some(first) = groups.first() else {
                    return unknown();
                };
                if groups.get(1).is_some_and(|second| {
                    (!majority || first.len() == second.len()) && minimum(first) == minimum(second)
                }) {
                    return CandidateFeature::Unknown {
                        reason: CandidateUnknownReason::Ambiguous,
                        source: None,
                    };
                }
                first
                    .iter()
                    .copied()
                    .min_by_key(|value| match value {
                        CandidateFeature::Identity { distance, .. } => *distance,
                        _ => 1001,
                    })
                    .cloned()
                    .unwrap_or_else(unknown)
            }
        }
    }
}

impl CandidateSampleVariant {
    pub(crate) fn effective(self, metric: RecognitionMatchMetric) -> Self {
        Self {
            template_metric: Some(self.template_metric.unwrap_or(metric)),
            ..self
        }
    }
    pub(crate) fn target(
        &self,
        target: &RecognitionTarget,
    ) -> RecognitionPackResult<RecognitionTarget> {
        let mut target = target.clone();
        if self.template_metric.is_some() && !matches!(target, RecognitionTarget::Template(_)) {
            return Err(RecognitionPackError::fatal(
                "template_metric sample parameter requires a template target",
            ));
        }
        match &mut target {
            RecognitionTarget::Template(target) => self.shift_region(&mut target.region)?,
            RecognitionTarget::Ocr(target) => self.shift_region(&mut target.region)?,
            RecognitionTarget::Color(target) => self.shift(&mut target.region)?,
            RecognitionTarget::ColorDigest(target) => self.shift(&mut target.region)?,
            _ => {
                return Err(RecognitionPackError::fatal(
                    "consensus supports OCR, template, color and color_digest targets",
                ));
            }
        }
        Ok(target)
    }

    fn shift(&self, rect: &mut PackRect) -> RecognitionPackResult<()> {
        rect.x = rect
            .x
            .checked_add(i32::from(self.dx))
            .ok_or_else(|| RecognitionPackError::fatal("sample x overflows"))?;
        rect.y = rect
            .y
            .checked_add(i32::from(self.dy))
            .ok_or_else(|| RecognitionPackError::fatal("sample y overflows"))?;
        Ok(())
    }

    fn shift_region(&self, region: &mut PackRegion) -> RecognitionPackResult<()> {
        if self.dx == 0 && self.dy == 0 {
            return Ok(());
        }
        match region {
            PackRegion::Rect(rect) => self.shift(rect),
            PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                offset,
                ..
            }) => {
                offset.x = offset
                    .x
                    .checked_add(i32::from(self.dx))
                    .ok_or_else(|| RecognitionPackError::fatal("sample relative x overflows"))?;
                offset.y = offset
                    .y
                    .checked_add(i32::from(self.dy))
                    .ok_or_else(|| RecognitionPackError::fatal("sample relative y overflows"))?;
                Ok(())
            }
            PackRegion::Keyword(_) => Err(RecognitionPackError::fatal(
                "jitter needs a rectangle or template_relative region",
            )),
        }
    }
}

impl SceneEvaluation<'_> {
    pub(crate) fn evaluate_candidate_sample(
        &self,
        target_id: &str,
        sample: CandidateSampleVariant,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let provider_ms = match self.evaluator().target(target_id)? {
            RecognitionTarget::Ocr(target) => target.timeout_ms,
            _ => 0,
        };
        if self.sample_deadline.is_some_and(|deadline| {
            std::time::Instant::now() >= deadline
                || std::time::Duration::from_millis(provider_ms)
                    > deadline.saturating_duration_since(std::time::Instant::now())
        }) {
            return Err(RecognitionPackError::fatal(
                "recognition sample budget insufficient before backend evaluation",
            ));
        }
        let target = sample.target(self.evaluator().target(target_id)?)?;
        let mut evaluator: RecognitionEvaluator = self.evaluator().clone();
        if let Some(metric) = sample.template_metric {
            evaluator.pack.defaults.match_metric = metric;
        }
        let context = evaluator.scene_context(self.scene());
        match target {
            RecognitionTarget::Template(target) => {
                evaluator.evaluate_template(self.scene(), &target)
            }
            RecognitionTarget::Ocr(target) => evaluator.evaluate_ocr(&context, &target),
            RecognitionTarget::Color(target) => evaluator.evaluate_color(self.scene(), &target),
            RecognitionTarget::ColorDigest(target) => {
                evaluator.evaluate_color_digest(self.scene(), &target)
            }
            _ => Err(RecognitionPackError::fatal(
                "unsupported consensus target kind",
            )),
        }
    }
}
