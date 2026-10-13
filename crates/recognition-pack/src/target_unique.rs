// SPDX-License-Identifier: AGPL-3.0-only

//! Unique matches (Workflow #396 REC-2, `contracts/resource-declarations.md`, section Task
//! schema `0.9` unique matches).
//!
//! A template target or a locate OCR target may declare `"unique": {"margin_milli": n}`. Wherever
//! the target is evaluated, a second place scoring at least the best match's score minus `n`
//! (in milli, rounded down) makes the target fail under `target_match_not_unique`, so that a
//! guard sends no input. A template that declares `unique` is scored by the exact score map of
//! `actingcommand-recognition`; its search limit is the existing 5 s, and a search past it is
//! the evaluation error `target_match_search_incomplete`.

use crate::codes::{RecognitionPackCode, RecognitionPackLocation, TargetMatchKind};
use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition as recognition;
use actingcommand_recognition::TemplateMatchTimingStage;

/// The declared margins: 1 to 1000 milli, written by the pack (no Runtime default).
const UNIQUE_MARGIN_MILLI: std::ops::RangeInclusive<i64> = 1..=1000;

/// Schema 0.7 `unique` of a template or locate OCR target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetUnique {
    pub margin_milli: u32,
}

/// The uniqueness check of a target that declares `unique`: the best match's score and the
/// margin in milli, and the second place within the margin when one exists (the target then
/// did not pass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TargetUniquenessEvidence {
    pub score_milli: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_score_milli: Option<u32>,
    pub margin_milli: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_rect: Option<PackRect>,
}

/// Records the uniqueness check on a target that otherwise passed. A second place makes it
/// fail under `target_match_not_unique`.
pub(crate) fn apply_uniqueness(
    evaluation: &mut TargetEvaluation,
    kind: TargetMatchKind,
    unique: TargetUnique,
    score_milli: u32,
    second: Option<(u32, PackRect)>,
) {
    evaluation.uniqueness = Some(TargetUniquenessEvidence {
        score_milli,
        second_score_milli: second.map(|(score, _)| score),
        margin_milli: unique.margin_milli,
        second_rect: second.map(|(_, rect)| rect),
    });
    if let Some((second_score_milli, _)) = second {
        evaluation.passed = false;
        evaluation.failure = Some(
            Outcome::new(RecognitionPackCode::TargetMatchNotUnique)
                .target_id(evaluation.id.as_str())
                .target_kind(kind)
                .score_milli(score_milli)
                .second_score_milli(second_score_milli)
                .margin_milli(unique.margin_milli)
                .stage(RecognitionPackLocation::TargetMatchUnique),
        );
        evaluation.message = "target match not unique".to_string();
    }
}

/// A template search's failure; a search past its limit carries `target_match_search_incomplete`.
pub(crate) fn search_error(
    target_id: &str,
    error: recognition::RecognitionError,
) -> RecognitionPackError {
    let incomplete = error.timing().is_some();
    let mut error = primitive_error(target_id, error);
    if incomplete {
        error.outcome = Some(
            Outcome::new(RecognitionPackCode::TargetMatchSearchIncomplete)
                .target_id(target_id)
                .timeout_ms(recognition::TEMPLATE_MATCH_TIMEOUT)
                .stage(RecognitionPackLocation::TargetMatchUnique),
        );
    }
    error
}

fn template_rect(matched: recognition::TemplateMatch) -> PackRect {
    PackRect {
        x: matched.x,
        y: matched.y,
        width: matched.width,
        height: matched.height,
    }
}

impl RecognitionEvaluator {
    /// A template target that declares `unique`, scored by the exact score map of its search
    /// region. Its best match and its pass decision follow the rules without `unique` on these
    /// exact scores; a target that passes is then checked for a second place.
    pub(crate) fn evaluate_unique_template(
        &self,
        context: &SceneEvaluation<'_>,
        target: &TemplateTarget,
        unique: TargetUnique,
        template_png: &[u8],
        region: Option<recognition::Rect>,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let scene = context.scene;
        let relative = target
            .color_check
            .as_ref()
            .is_some_and(|check| matches!(check.region, PackRegion::TemplateRelative(_)));
        let map = scene
            .exact_score_map(
                template_png,
                region,
                self.default_match_metric(),
                if relative {
                    TemplateMatchTimingStage::JointTemplateColor
                } else {
                    TemplateMatchTimingStage::Exact
                },
                &mut context.spectra.borrow_mut(),
                &self.search_clock,
            )
            .map_err(|error| search_error(&target.id, error))?;
        if relative {
            let (mut evaluation, accepted) =
                self.joint_color_evaluation(scene, target, JointSearch::Map(&map))?;
            if let Some(accepted) = accepted {
                let second = map
                    .second_within(accepted, unique.margin_milli, |candidate| {
                        Ok(self
                            .relative_color_at(scene, target, candidate)?
                            .0
                            .is_some_and(|color| color.distance <= color.max_distance))
                    })
                    .map_err(|error| search_error(&target.id, error))?;
                apply_uniqueness(
                    &mut evaluation,
                    TargetMatchKind::Template,
                    unique,
                    recognition::score_milli(accepted.score),
                    second.map(|second| {
                        (
                            recognition::score_milli(second.score),
                            template_rect(second),
                        )
                    }),
                );
            }
            return Ok(evaluation);
        }

        let best = map
            .best()
            .map_err(|error| search_error(&target.id, error))?;
        let threshold = target
            .threshold
            .unwrap_or(self.pack.defaults.template_threshold);
        let template_ok = best.score >= threshold;
        let color = match &target.color_check {
            Some(check) => Some(self.evaluate_color_check(scene, &target.id, check)?),
            None => None,
        };
        let color_ok = color
            .as_ref()
            .is_none_or(|color| color.distance <= color.max_distance);
        let mut evaluation = TargetEvaluation {
            id: target.id.clone(),
            kind: TargetKind::Template,
            sample_evaluations: Vec::new(),
            sampling: None,
            passed: template_ok && color_ok,
            template: Some(TemplateEvaluation {
                x: best.x,
                y: best.y,
                width: best.width,
                height: best.height,
                raw_score: best.raw_score,
                score: best.score,
                threshold,
            }),
            color,
            ocr: None,
            nn: None,
            color_digest: None,
            composite: None,
            failure: None,
            uniqueness: None,
            message: template_message(template_ok, color_ok),
        };
        if evaluation.passed {
            // An absolute color check holds for every position alike.
            let second = map
                .second_within(best, unique.margin_milli, |_| Ok(true))
                .map_err(|error| search_error(&target.id, error))?;
            apply_uniqueness(
                &mut evaluation,
                TargetMatchKind::Template,
                unique,
                recognition::score_milli(best.score),
                second.map(|second| {
                    (
                        recognition::score_milli(second.score),
                        template_rect(second),
                    )
                }),
            );
        }
        Ok(evaluation)
    }
}

/// The wire shape of a schema 0.7 `unique` object on the target at `index`: a template target,
/// or an OCR target that declares `locate`, with `margin_milli` in 1..=1000.
pub(crate) fn validate_unique_wire(
    schema: &str,
    target: &Map<String, Value>,
    index: usize,
) -> RecognitionPackResult<()> {
    let Some(value) = target.get("unique") else {
        return Ok(());
    };
    let pointer = format!("/targets/{index}/unique");
    let refuse = |pointer: String, reason: ResourceDeclarationReason, explanation: &str| {
        RecognitionPackError::fatal(explanation).at_declaration(pointer, reason)
    };
    if schema != SCHEMA_0_7 {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::UnconsumedField,
            "unique requires schema_version '0.7'",
        ));
    }
    if target.get("type").and_then(Value::as_str) == Some("ocr") && !target.contains_key("locate") {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::UnconsumedField,
            "unique on an OCR target requires locate",
        ));
    }
    let unique = validate_strict_object(value, &["margin_milli"], &pointer)?;
    let margin_pointer = format!("{pointer}/margin_milli");
    let Some(margin) = unique.get("margin_milli") else {
        return Err(refuse(
            margin_pointer,
            ResourceDeclarationReason::MissingField,
            "unique lacks margin_milli",
        ));
    };
    match margin.as_i64() {
        Some(margin) if UNIQUE_MARGIN_MILLI.contains(&margin) => Ok(()),
        _ if margin.is_i64() || margin.is_u64() => Err(refuse(
            margin_pointer,
            ResourceDeclarationReason::InvalidValue,
            "unique.margin_milli must be in 1..=1000",
        )),
        _ => Err(refuse(
            margin_pointer,
            ResourceDeclarationReason::InvalidType,
            "unique.margin_milli must be an integer",
        )),
    }
}

/// The declaration rules of `unique` on a parsed pack: schema 0.7, a template target or an OCR
/// target that declares `locate`, `margin_milli` in 1..=1000 (the wire shape is checked by
/// [`validate_unique_wire`] when the pack is loaded from JSON), and no such template as a
/// candidate layout feature read at an offset or as a `repeated_anchor` anchor.
pub(crate) fn validate_unique_declarations(pack: &RecognitionPack, errors: &mut Vec<String>) {
    for (index, target) in pack.targets.iter().enumerate() {
        let (unique, ocr_without_locate) = match target {
            RecognitionTarget::Template(target) => (target.unique, false),
            RecognitionTarget::Ocr(target) => (target.unique, target.locate.is_none()),
            _ => continue,
        };
        let Some(unique) = unique else {
            continue;
        };
        if pack.schema_version != SCHEMA_0_7 {
            errors.push(format!(
                "target[{index}].unique requires schema_version '{SCHEMA_0_7}'"
            ));
        }
        if ocr_without_locate {
            errors.push(format!(
                "target[{index}].unique on an OCR target requires locate"
            ));
        }
        if !UNIQUE_MARGIN_MILLI.contains(&i64::from(unique.margin_milli)) {
            errors.push(format!(
                "target[{index}].unique.margin_milli must be in 1..=1000"
            ));
        }
    }
    // Workflow #308 SEL-L1 merge: a feature read at an offset is evaluated over one moved
    // region and a `repeated_anchor` anchor takes every accepted match; neither checks a second
    // place, so neither may name a template that declares `unique`.
    let declares_unique = |target_id: &str| {
        pack.targets.iter().any(|target| {
            matches!(target, RecognitionTarget::Template(template)
                if template.id == target_id && template.unique.is_some())
        })
    };
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        if let Some(anchor) = layout.anchor.as_deref()
            && declares_unique(anchor)
        {
            errors.push(format!(
                "/candidate_layouts/{layout_index}/anchor: template '{anchor}' declares unique, which a repeated_anchor anchor does not check"
            ));
        }
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let (Some(target_id), Some(_)) = (&feature.target, feature.offset)
                && declares_unique(target_id.as_str())
            {
                errors.push(format!(
                    "/candidate_layouts/{layout_index}/features/{feature_index}/target: template '{target_id}' declares unique, which a feature read at an offset does not check"
                ));
            }
        }
    }
}

/// The second place of a locate target's hit: another block whose text matches (a hit or a
/// weak hit), with a confidence in milli (0 when absent) of at least the hit's minus the
/// margin; the highest such, the earlier block on a tie. Cells are not considered.
pub(crate) fn second_text_block(
    blocks: &[OcrTextEvidence],
    target: &OcrTarget,
    hit_index: usize,
    hit_milli: u32,
    unique: TargetUnique,
) -> Option<(u32, PackRect)> {
    let floor = i64::from(hit_milli) - i64::from(unique.margin_milli);
    let mut second: Option<(u32, PackRect)> = None;
    for (index, block) in blocks.iter().enumerate() {
        if index == hit_index || crate::text_locate::block_match(block, target).is_none() {
            continue;
        }
        let milli =
            u32::from(actingcommand_contract::ocr_confidence_milli(block.confidence).unwrap_or(0));
        if i64::from(milli) >= floor && second.is_none_or(|(best, _)| milli > best) {
            second = Some((milli, block.rect));
        }
    }
    second
}

#[cfg(test)]
#[path = "target_unique_tests.rs"]
mod tests;
