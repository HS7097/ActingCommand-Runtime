// SPDX-License-Identifier: AGPL-3.0-only

//! Brightness and contrast floors of template targets (Workflow #411 B13, B-train plan §2.10
//! with D9; `contracts/resource-declarations.md`, section Task schema `0.9` brightness and
//! contrast floors).
//!
//! A schema `0.7` template target may declare
//! `"luma_floor": {"mean_ratio_milli"?, "contrast_ratio_milli"?}`. The window a candidate
//! matched is read over the template's scored pixels (`actingcommand-recognition`,
//! `LumaReference`): its mean and its standard deviation, each divided by the template's own and
//! times 1000, rounded down. A candidate below a declared floor is not accepted: the floor joins
//! the joint acceptance of the template search (`template-relative-color.md`), so a target whose
//! candidates all fall below it does not pass, with no outcome code of its own. The template
//! evidence carries the chosen candidate's ratios as `luma` (ledger revision 4, register §1.153
//! LR-2) when, and only when, the target declares the floor.

use crate::*;
use actingcommand_contract::{ResourceDeclarationReason, TaskDiagnosticLumaData};
use actingcommand_recognition as recognition;

/// The floor members, in their wire order.
const LUMA_FLOOR_MEMBERS: [&str; 2] = ["mean_ratio_milli", "contrast_ratio_milli"];

/// Schema 0.7 `luma_floor` of a template target: the least mean ratio and the least contrast
/// ratio, in milli, a candidate's window must reach. An absent member sets no floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateLumaFloor {
    #[serde(default)]
    pub mean_ratio_milli: Option<u32>,
    #[serde(default)]
    pub contrast_ratio_milli: Option<u32>,
}

impl TemplateLumaFloor {
    /// Whether `luma` reaches every declared floor.
    pub fn admits(&self, luma: TaskDiagnosticLumaData) -> bool {
        // Red proof 1 mutation: the mean floor is ignored.
        self.contrast_ratio_milli
            .is_none_or(|floor| luma.contrast_ratio_milli >= floor)
    }
}

/// The floor of one template target with its template's references, one per factor the
/// template is searched at.
pub(crate) struct LumaCheck {
    floor: TemplateLumaFloor,
    references: Vec<(u32, recognition::LumaReference)>,
}

impl LumaCheck {
    /// The check of `target`, whose template is `template_png` scored over the pixels `mask`
    /// selects; `None` when the target declares no `luma_floor`.
    pub(crate) fn of(
        target: &TemplateTarget,
        template_png: &[u8],
        mask: Option<recognition::TemplateMask>,
    ) -> RecognitionPackResult<Option<Self>> {
        let Some(floor) = target.luma_floor else {
            return Ok(None);
        };
        let references = crate::template_scales::searched_factors(target)
            .into_iter()
            .map(|factor| {
                recognition::LumaReference::of_png_scaled(template_png, mask, factor)
                    .map(|reference| (factor, reference))
            })
            .collect::<recognition::RecognitionResult<Vec<_>>>()
            .map_err(|err| primitive_error(&target.id, err))?;
        Ok(Some(Self { floor, references }))
    }

    /// The ratios of the window `matched` covers, a match of the template resampled by
    /// `scale_milli`.
    pub(crate) fn measure(
        &self,
        scene: &Scene,
        matched: recognition::TemplateMatch,
        scale_milli: u32,
    ) -> recognition::RecognitionResult<TaskDiagnosticLumaData> {
        let Some((_, reference)) = self
            .references
            .iter()
            .find(|(factor, _)| *factor == scale_milli)
        else {
            return Err(recognition::RecognitionError::fatal(format!(
                "no luma reference for the template scaled by {scale_milli} milli"
            )));
        };
        let ratios = scene.luma_ratios(reference, matched)?;
        Ok(TaskDiagnosticLumaData {
            mean_ratio_milli: ratios.mean_ratio_milli,
            contrast_ratio_milli: ratios.contrast_ratio_milli,
        })
    }

    /// Whether `luma` reaches the target's floor.
    pub(crate) fn admits(&self, luma: TaskDiagnosticLumaData) -> bool {
        self.floor.admits(luma)
    }
}

/// The wire shape of a schema 0.7 `luma_floor` object on the template target at `index`: an
/// object of `mean_ratio_milli` and `contrast_ratio_milli`, each optional, each an integer in
/// `0..=u32::MAX`.
pub(crate) fn validate_luma_floor_wire(
    schema: &str,
    target: &Map<String, Value>,
    index: usize,
) -> RecognitionPackResult<()> {
    let Some(value) = target.get("luma_floor") else {
        return Ok(());
    };
    let pointer = format!("/targets/{index}/luma_floor");
    let refuse = |pointer: String, reason: ResourceDeclarationReason, explanation: &str| {
        RecognitionPackError::fatal(explanation).at_declaration(pointer, reason)
    };
    if schema != SCHEMA_0_7 {
        return Err(refuse(
            pointer,
            ResourceDeclarationReason::UnconsumedField,
            "luma_floor requires schema_version '0.7'",
        ));
    }
    let floor = validate_strict_object(value, &LUMA_FLOOR_MEMBERS, &pointer)?;
    for member in LUMA_FLOOR_MEMBERS {
        let Some(value) = floor.get(member) else {
            continue;
        };
        let member_pointer = format!("{pointer}/{member}");
        match value.as_u64() {
            Some(milli) if u32::try_from(milli).is_ok() => {}
            _ if value.is_i64() || value.is_u64() => {
                return Err(refuse(
                    member_pointer,
                    ResourceDeclarationReason::InvalidValue,
                    "a luma floor is in 0..=4294967295 milli",
                ));
            }
            _ => {
                return Err(refuse(
                    member_pointer,
                    ResourceDeclarationReason::InvalidType,
                    "a luma floor is an integer",
                ));
            }
        }
    }
    Ok(())
}

/// The declaration rules of `luma_floor` on a parsed pack (the wire shape is checked by
/// [`validate_luma_floor_wire`] when the pack is loaded from JSON): schema 0.7, and no such
/// template as a `repeated_anchor` anchor or a candidate layout feature read at an offset, which
/// accept a match without the floor.
pub(crate) fn validate_luma_floor_declarations(pack: &RecognitionPack, errors: &mut Vec<String>) {
    let mut floored = HashSet::new();
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::Template(target) = target else {
            continue;
        };
        if target.luma_floor.is_none() {
            continue;
        }
        floored.insert(target.id.as_str());
        if pack.schema_version != SCHEMA_0_7 {
            errors.push(format!(
                "target[{index}].luma_floor requires schema_version '{SCHEMA_0_7}'"
            ));
        }
    }
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        if let Some(anchor) = layout.anchor.as_deref()
            && floored.contains(anchor)
        {
            errors.push(format!(
                "/candidate_layouts/{layout_index}/anchor: template '{anchor}' declares luma_floor, which a repeated_anchor anchor does not check"
            ));
        }
        for (feature_index, feature) in layout.features.iter().enumerate() {
            if let (Some(target_id), Some(_)) = (&feature.target, feature.offset)
                && floored.contains(target_id.as_str())
            {
                errors.push(format!(
                    "/candidate_layouts/{layout_index}/features/{feature_index}/target: template '{target_id}' declares luma_floor, which a feature read at an offset does not check"
                ));
            }
        }
    }
}
