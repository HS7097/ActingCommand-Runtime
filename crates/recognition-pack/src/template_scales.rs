// SPDX-License-Identifier: AGPL-3.0-only

//! Multi-scale templates (Workflow #396 REC-5, REC-M v1.2.1 §7;
//! `contracts/resource-declarations.md`, section Task schema `0.9` multi-scale templates).
//!
//! A schema `0.7` template target may declare `"scales": {"factors_milli": [...]}`: 1 to 4
//! distinct factors from 500 to 2000, in the order that breaks ties. The template is resampled by
//! each factor with the primitive of `actingcommand-recognition` and every resampled template is
//! searched over the target's region on the exact score map; the highest score wins, an equal one
//! the earlier factor. The match's box is the resampled template's, and the factor it hit at
//! (`scale_milli`) scales the package lengths that hang on the match: a relative color region, a
//! `template_relative` OCR region and a click `offset`, each `(v * factor + 500) div 1000` with
//! the sign of `v`, a width or height at least 1. Factors describe the interface's own zoom in the
//! package's base coordinates, never a device resolution.

use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition::resample::{self, SCALE_IDENTITY_MILLI};

/// How many factors a target declares.
const SCALE_FACTOR_COUNT: std::ops::RangeInclusive<usize> = 1..=4;

/// The factors a target may declare, in milli.
const SCALE_FACTOR_MILLI: std::ops::RangeInclusive<u64> = 500..=2000;

/// The fewest pixels each side of a resampled template keeps (a contract constant).
pub const TEMPLATE_SCALED_MIN_SIDE: u32 = 4;

/// Schema 0.7 `scales` of a template target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateScales {
    pub factors_milli: Vec<u32>,
}

/// The factors a template target is searched at: its declared ones, or 1000 alone.
pub(crate) fn searched_factors(target: &TemplateTarget) -> Vec<u32> {
    target.scales.as_ref().map_or_else(
        || vec![SCALE_IDENTITY_MILLI],
        |scales| scales.factors_milli.clone(),
    )
}

/// The relative region `offset`, `width` by `height` of a template that matched at
/// `scale_milli`, scaled as the declaration's lengths scale; `None` when a length leaves the
/// `i32` range.
pub(crate) fn scaled_relative(
    offset: OcrRegionOffset,
    width: i32,
    height: i32,
    scale_milli: u32,
) -> Option<(OcrRegionOffset, i32, i32)> {
    if scale_milli == SCALE_IDENTITY_MILLI {
        return Some((offset, width, height));
    }
    Some((
        OcrRegionOffset {
            x: resample::scale_signed(offset.x, scale_milli)?,
            y: resample::scale_signed(offset.y, scale_milli)?,
        },
        resample::scale_signed(width, scale_milli)?.max(1),
        resample::scale_signed(height, scale_milli)?.max(1),
    ))
}

/// The wire shape of a schema 0.7 `scales` object on the target at `index`:
/// `{"factors_milli": [...]}` with 1 to 4 distinct integers from 500 to 2000.
pub(crate) fn validate_scales_wire(
    schema: &str,
    target: &Map<String, Value>,
    index: usize,
) -> RecognitionPackResult<()> {
    let Some(value) = target.get("scales") else {
        return Ok(());
    };
    let pointer = format!("/targets/{index}/scales");
    let refuse = |pointer: String, reason: ResourceDeclarationReason, explanation: &str| {
        RecognitionPackError::fatal(explanation).at_declaration(pointer, reason)
    };
    if schema != SCHEMA_0_7 {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::UnconsumedField,
            "scales requires schema_version '0.7'",
        ));
    }
    let scales = validate_strict_object(value, &["factors_milli"], &pointer)?;
    let factors_pointer = format!("{pointer}/factors_milli");
    let Some(factors) = scales.get("factors_milli") else {
        return Err(refuse(
            factors_pointer,
            ResourceDeclarationReason::MissingField,
            "scales lacks factors_milli",
        ));
    };
    let Some(factors) = factors.as_array() else {
        return Err(refuse(
            factors_pointer,
            ResourceDeclarationReason::InvalidType,
            "scales.factors_milli is an array",
        ));
    };
    if !SCALE_FACTOR_COUNT.contains(&factors.len()) {
        return Err(refuse(
            factors_pointer,
            ResourceDeclarationReason::InvalidValue,
            "scales.factors_milli holds 1 to 4 factors",
        ));
    }
    let mut seen = Vec::with_capacity(factors.len());
    for (position, factor) in factors.iter().enumerate() {
        let factor_pointer = format!("{factors_pointer}/{position}");
        match factor.as_u64() {
            Some(milli) if SCALE_FACTOR_MILLI.contains(&milli) && !seen.contains(&milli) => {
                seen.push(milli);
            }
            Some(milli) if SCALE_FACTOR_MILLI.contains(&milli) => {
                return Err(refuse(
                    factor_pointer,
                    ResourceDeclarationReason::InvalidValue,
                    "scales.factors_milli repeats a factor",
                ));
            }
            _ if factor.is_i64() || factor.is_u64() => {
                return Err(refuse(
                    factor_pointer,
                    ResourceDeclarationReason::InvalidValue,
                    "a scale factor is in 500..=2000 milli",
                ));
            }
            _ => {
                return Err(refuse(
                    factor_pointer,
                    ResourceDeclarationReason::InvalidType,
                    "a scale factor is an integer",
                ));
            }
        }
    }
    Ok(())
}

/// The declaration rules of `scales` on a parsed pack (the wire shape is checked by
/// [`validate_scales_wire`] when the pack is loaded from JSON): schema 0.7, 1 to 4 distinct
/// factors from 500 to 2000. A target that declares them is never a `target_consensus` target
/// (each frame would take its own factor), a `repeated_anchor` anchor or a candidate layout
/// feature read at an offset (both search the template as declared).
pub(crate) fn validate_scales_declarations(pack: &RecognitionPack, errors: &mut Vec<String>) {
    let mut scaled = HashSet::new();
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::Template(target) = target else {
            continue;
        };
        let Some(scales) = &target.scales else {
            continue;
        };
        scaled.insert(target.id.as_str());
        if pack.schema_version != SCHEMA_0_7 {
            errors.push(format!(
                "target[{index}].scales requires schema_version '{SCHEMA_0_7}'"
            ));
        }
        if !SCALE_FACTOR_COUNT.contains(&scales.factors_milli.len()) {
            errors.push(format!(
                "target[{index}].scales.factors_milli must hold 1 to 4 factors"
            ));
        }
        for (position, factor) in scales.factors_milli.iter().enumerate() {
            if !SCALE_FACTOR_MILLI.contains(&u64::from(*factor)) {
                errors.push(format!(
                    "target[{index}].scales.factors_milli[{position}] must be in 500..=2000"
                ));
            }
            if scales.factors_milli[..position].contains(factor) {
                errors.push(format!(
                    "target[{index}].scales.factors_milli[{position}] repeats a factor"
                ));
            }
        }
    }
    for target_id in pack.target_consensus.keys() {
        if scaled.contains(target_id.as_str()) {
            errors.push(format!(
                "target_consensus/{target_id}: template '{target_id}' declares scales, which target consensus does not take"
            ));
        }
    }
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        if let Some(anchor) = layout.anchor.as_deref()
            && scaled.contains(anchor)
        {
            errors.push(format!(
                "/candidate_layouts/{layout_index}/anchor: template '{anchor}' declares scales, which a repeated_anchor anchor does not search"
            ));
        }
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let (Some(target_id), Some(_)) = (&feature.target, feature.offset)
                && scaled.contains(target_id.as_str())
            {
                errors.push(format!(
                    "/candidate_layouts/{layout_index}/features/{feature_index}/target: template '{target_id}' declares scales, which a feature read at an offset does not search"
                ));
            }
        }
    }
}

/// The size of the region a template target searches: its rectangle, or the coordinate space
/// for `full_frame`; `None` when the pack does not say.
pub(crate) fn search_size(pack: &RecognitionPack, target: &TemplateTarget) -> Option<(u32, u32)> {
    match &target.region {
        PackRegion::Rect(rect) => Some((
            u32::try_from(rect.width).ok()?,
            u32::try_from(rect.height).ok()?,
        )),
        PackRegion::Keyword(keyword) if keyword == "full_frame" => pack
            .coordinate_space
            .map(|space| (space.width, space.height)),
        _ => None,
    }
}

#[cfg(test)]
#[path = "template_scales_tests.rs"]
mod tests;
