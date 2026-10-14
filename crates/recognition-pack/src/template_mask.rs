// SPDX-License-Identifier: AGPL-3.0-only

//! Template masks and the `ccorr_normed` threshold floor (Workflow #396 REC-7,
//! `contracts/resource-declarations.md`, section Pack schema `0.7` template masks and the
//! threshold floor).
//!
//! A schema `0.7` template target may declare `"mask": {"type": "range", "lower", "upper"}`:
//! only the template pixels whose gray value lies in the range are scored, by the exact score
//! map of `actingcommand-recognition`. Admission reads every template it can evaluate: a mask
//! selects at least [`TEMPLATE_MASK_MIN_PIXELS`] pixels (with some variance under
//! `ccoeff_normed`), and a `ccorr_normed` threshold above 0 lies at least
//! [`TEMPLATE_FLAT_FIELD_MARGIN_MILLI`] above the template's flat-field score, its score on any
//! region of one non-zero brightness.

use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition as recognition;

/// The fewest template pixels a mask selects (a contract constant).
pub const TEMPLATE_MASK_MIN_PIXELS: u64 = 16;

/// How far, in milli, a `ccorr_normed` template's threshold lies at least above its flat-field
/// score in milli (a contract constant).
pub const TEMPLATE_FLAT_FIELD_MARGIN_MILLI: u32 = 50;

/// The mask a template target's search scores with: its gray range, in a schema `0.7` pack. An
/// older schema's mask is an unsupported semantic ([`UnsupportedRecognitionTarget`]), and
/// admission refuses any other mask in schema `0.7`.
pub(crate) fn scored_mask(
    schema_version: &str,
    target: &TemplateTarget,
) -> Option<recognition::TemplateMask> {
    match &target.mask {
        Some(RecognitionMask::Range { lower, upper }) if schema_version == SCHEMA_0_7 => {
            Some(recognition::TemplateMask {
                lower: *lower,
                upper: *upper,
            })
        }
        _ => None,
    }
}

/// The wire shape of a schema `0.7` template's `mask` on the target at `index`:
/// `{"type": "range", "lower", "upper"}` with integer bounds in 0..=255 and `lower <= upper`.
/// `null` is no mask.
pub(crate) fn validate_mask_wire(
    target: &Map<String, Value>,
    index: usize,
) -> RecognitionPackResult<()> {
    let Some(mask) = target.get("mask").filter(|mask| !mask.is_null()) else {
        return Ok(());
    };
    let pointer = format!("/targets/{index}/mask");
    let refuse = |pointer: String, reason: ResourceDeclarationReason, explanation: &str| {
        RecognitionPackError::fatal(explanation).at_declaration(pointer, reason)
    };
    let Some(object) = mask.as_object() else {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::InvalidType,
            "a template mask is an object",
        ));
    };
    let type_pointer = format!("{pointer}/type");
    match object.get("type") {
        Some(Value::String(kind)) if kind == "range" => {}
        Some(Value::String(_)) => {
            return Err(refuse(
                type_pointer,
                ResourceDeclarationReason::InvalidValue,
                "a schema 0.7 template mask is a gray range",
            ));
        }
        Some(_) => {
            return Err(refuse(
                type_pointer,
                ResourceDeclarationReason::InvalidType,
                "a template mask type is a string",
            ));
        }
        None => {
            return Err(refuse(
                type_pointer,
                ResourceDeclarationReason::MissingField,
                "a template mask lacks type",
            ));
        }
    }
    reject_unknown_fields(object, &["type", "lower", "upper"], &pointer)?;
    let mut bounds = [0_u64; 2];
    for (bound, field) in bounds.iter_mut().zip(["lower", "upper"]) {
        let bound_pointer = format!("{pointer}/{field}");
        let Some(value) = object.get(field) else {
            return Err(refuse(
                bound_pointer,
                ResourceDeclarationReason::MissingField,
                "a template mask lacks a bound",
            ));
        };
        match value.as_u64() {
            Some(value) if value <= u64::from(u8::MAX) => *bound = value,
            _ if value.is_i64() || value.is_u64() => {
                return Err(refuse(
                    bound_pointer,
                    ResourceDeclarationReason::InvalidValue,
                    "a template mask bound is in 0..=255",
                ));
            }
            _ => {
                return Err(refuse(
                    bound_pointer,
                    ResourceDeclarationReason::InvalidType,
                    "a template mask bound is an integer",
                ));
            }
        }
    }
    if bounds[0] > bounds[1] {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::InvalidValue,
            "a template mask's lower bound exceeds its upper bound",
        ));
    }
    Ok(())
}

/// The mask rules of a parsed pack (the wire shape is checked by [`validate_mask_wire`] when the
/// pack is loaded from JSON): schema `0.7` scores only a gray range whose `lower` does not exceed
/// its `upper`. A `repeated_anchor` anchor and a candidate layout feature read at an offset are
/// searched over every template pixel, so neither may name a masked template.
pub(crate) fn validate_mask_declarations(pack: &RecognitionPack, errors: &mut Vec<String>) {
    if pack.schema_version != SCHEMA_0_7 {
        return;
    }
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::Template(target) = target else {
            continue;
        };
        match &target.mask {
            Some(RecognitionMask::Range { lower, upper }) if lower > upper => errors.push(format!(
                "target[{index}].mask lower bound exceeds its upper bound"
            )),
            Some(RecognitionMask::Bitmap { .. }) => errors.push(format!(
                "target[{index}].mask must be a gray range in schema_version '{SCHEMA_0_7}'"
            )),
            _ => {}
        }
    }
    let masked = |target_id: &str| {
        pack.targets.iter().any(|target| {
            matches!(target, RecognitionTarget::Template(template)
                if template.id == target_id && template.mask.is_some())
        })
    };
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        if let Some(anchor) = layout.anchor.as_deref()
            && masked(anchor)
        {
            errors.push(format!(
                "/candidate_layouts/{layout_index}/anchor: template '{anchor}' declares mask, which a repeated_anchor anchor does not score"
            ));
        }
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let (Some(target_id), Some(_)) = (&feature.target, feature.offset)
                && masked(target_id.as_str())
            {
                errors.push(format!(
                    "/candidate_layouts/{layout_index}/features/{feature_index}/target: template '{target_id}' declares mask, which a feature read at an offset does not score"
                ));
            }
        }
    }
}

/// A flat-field score in milli, rounded down as every score is.
fn flat_score_milli(score: f64) -> u32 {
    let milli = (score * 1000.0).floor();
    if milli.is_nan() || milli <= 0.0 {
        0
    } else if milli >= 1000.0 {
        1000
    } else {
        milli as u32
    }
}

impl RecognitionEvaluator {
    /// Workflow #396 REC-7: the admission rules that read each template's pixels.
    ///
    /// - A mask selects at least [`TEMPLATE_MASK_MIN_PIXELS`] template pixels, and under
    ///   `ccoeff_normed` they are not all of one value.
    /// - Under `ccorr_normed`, a threshold above 0 is at least the template's flat-field score in
    ///   milli (rounded down, over the masked pixels when it declares a mask) plus
    ///   [`TEMPLATE_FLAT_FIELD_MARGIN_MILLI`]. A threshold of 0 declares a target that always
    ///   passes and is not checked. `ccoeff_normed` scores a region of one brightness 0.
    ///
    /// Targets this build cannot evaluate are skipped. A refusal is `InvalidValue` at the mask
    /// (`/targets/<i>/mask`) or at the threshold in effect (`/targets/<i>/threshold`, or
    /// `/defaults/template_threshold` when the target declares none); the developer text of a
    /// floor refusal names every template below its floor with its `flat_score_milli`. Pack
    /// containment runs this where it admits a package, holding its template bytes (the Runtime
    /// and `actinglab package bundle`); metadata-only validation reads no template.
    pub fn admit_template_assets(&self) -> RecognitionPackResult<()> {
        let pack = &self.pack;
        let ccorr = pack.defaults.match_metric == RecognitionMatchMetric::CcorrNormed;
        let refuse = |pointer: String, explanation: String| {
            let mut error = RecognitionPackError::fatal(explanation)
                .at_declaration(pointer, ResourceDeclarationReason::InvalidValue);
            if let Some(issue) = &mut error.declaration_issue {
                issue.schema_version = Some(pack.schema_version.clone());
            }
            error
        };
        // The first refusal's pointer, and every template below its floor.
        let mut first_pointer = None;
        let mut below_floor = Vec::new();
        for (index, target) in pack.targets.iter().enumerate() {
            let RecognitionTarget::Template(target) = target else {
                continue;
            };
            if UnsupportedRecognitionTarget::of_template(target, &pack.schema_version).is_some() {
                continue;
            }
            let mask = scored_mask(&pack.schema_version, target);
            let threshold = target.threshold.unwrap_or(pack.defaults.template_threshold);
            let floored = ccorr && threshold > 0.0;
            if mask.is_none() && !floored {
                continue;
            }
            let template_png = self
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
            let sums = recognition::TemplatePixelSums::of_png(&template_png, mask)
                .map_err(|err| primitive_error(&target.id, err))?;
            if mask.is_some() {
                if sums.count < TEMPLATE_MASK_MIN_PIXELS {
                    return Err(refuse(
                        format!("/targets/{index}/mask"),
                        format!(
                            "template target '{}' mask selects {} pixels, fewer than {TEMPLATE_MASK_MIN_PIXELS}",
                            target.id, sums.count
                        ),
                    ));
                }
                if !ccorr && !sums.has_variance() {
                    return Err(refuse(
                        format!("/targets/{index}/mask"),
                        format!(
                            "ccoeff_normed template target '{}' has no variance inside its mask",
                            target.id
                        ),
                    ));
                }
            }
            if floored {
                let flat_score_milli = flat_score_milli(sums.flat_field_score());
                let floor_milli = flat_score_milli + TEMPLATE_FLAT_FIELD_MARGIN_MILLI;
                // The threshold and the floor compare as the evaluation compares scores (f32);
                // a declared threshold of three decimals equals its milli value exactly.
                if threshold < floor_milli as f32 / 1000.0 {
                    let pointer = if target.threshold.is_some() {
                        format!("/targets/{index}/threshold")
                    } else {
                        "/defaults/template_threshold".to_string()
                    };
                    below_floor.push(format!(
                        "template target '{}' threshold {threshold} at {pointer} is below its flat-field floor: flat_score_milli={flat_score_milli}, floor_milli={floor_milli}",
                        target.id
                    ));
                    if first_pointer.is_none() {
                        first_pointer = Some(pointer);
                    }
                }
            }
        }
        match first_pointer {
            Some(pointer) => Err(refuse(pointer, below_floor.join("; "))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "template_mask_tests.rs"]
mod tests;
