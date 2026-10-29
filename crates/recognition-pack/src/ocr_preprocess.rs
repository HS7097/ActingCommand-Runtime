// SPDX-License-Identifier: AGPL-3.0-only

//! Recognition preprocessing on OCR reads (Workflow #411 A58, #428; design CF v2 §5.4 to §5.8,
//! slice CFL-2, since 0.12.29; `contracts/recognition-preprocess.md`, section Hand-over).
//!
//! An OCR target or a layout's `shared_ocr` may declare `preprocess`, one fixed chain of
//! `actingcommand_recognition::preprocess`. This module holds its wire form (the JSON parser and
//! writer, which round-trip), the hand-over of one read and the evidence of one application.
//!
//! The hand-over: a read without a chain hands the provider the frame itself, its region and its
//! block mode, byte for byte as before. A read with a chain resolves its region on the frame
//! first, runs the chain over the region's RGB pixels exactly once, and hands the provider only
//! the processed region as a small RGB8 frame (R = G = B) with the region `(0, 0, w, h)` and an
//! explicit block mode (`Implicit` becomes `Region`; `Region` and `Detect` stay). The blocks that
//! come back are moved back onto the region's origin. The chain's evidence goes into read-only
//! answers, never into the ledger.

use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition::preprocess::{
    ColorSpace, MAX_STEPS, OTSU, PREPROCESS_ALGORITHM, Preprocess, PreprocessKind, PreprocessStep,
    ThresholdValue,
};
use serde::{Deserializer, Serializer};
use std::borrow::Cow;

/// A declared `preprocess`, validated: its chain. Two declarations are the same chain exactly
/// when their steps are equal, the colors of `keep_colors` in their declared order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OcrPreprocess(Preprocess);

impl OcrPreprocess {
    /// The declaration `value` (`{"steps": [...]}`), or the first refusal of its shape, members
    /// or chain, with its pointer below `preprocess`.
    pub fn from_json(value: &Value) -> Result<Self, PreprocessRefusal> {
        parse_preprocess(value)
    }

    /// The chain.
    pub fn chain(&self) -> &Preprocess {
        &self.0
    }

    /// The declaration as JSON, `{"steps": [...]}`: every step in its declared order, `kind`
    /// first and then its members, so that [`Self::from_json`] reads it back unchanged.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("steps".to_owned(), steps_json(self.0.steps()));
        Value::Object(object)
    }

    /// The index of the chain's `threshold` step with the value `"otsu"`, the one step that
    /// reads its whole region; `None` when every step works pixel by pixel.
    pub fn otsu_step(&self) -> Option<usize> {
        self.0.steps().iter().position(|step| {
            matches!(
                step,
                PreprocessStep::Threshold {
                    value: ThresholdValue::Otsu
                }
            )
        })
    }
}

impl<'de> Deserialize<'de> for OcrPreprocess {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        parse_preprocess(&value).map_err(|refusal| serde::de::Error::custom(refusal.explanation))
    }
}

/// A refused `preprocess` declaration: the JSON pointer below the `preprocess` object (empty for
/// the object itself), the declaration reason and a developer explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocessRefusal {
    pub pointer: String,
    pub reason: ResourceDeclarationReason,
    pub explanation: String,
}

fn refusal(
    pointer: impl Into<String>,
    reason: ResourceDeclarationReason,
    explanation: impl Into<String>,
) -> PreprocessRefusal {
    PreprocessRefusal {
        pointer: pointer.into(),
        reason,
        explanation: explanation.into(),
    }
}

/// The declaration's shape first (`contracts/recognition-preprocess.md`, Refusals): an object
/// whose only member is `steps`, an array of 1 to 4 steps, each an object with a known `kind`
/// and exactly that kind's members, each of its type and range. Then the chain's own syntax
/// (`Preprocess::new`), whose refusal is `InvalidValue` at its pointer.
fn parse_preprocess(value: &Value) -> Result<OcrPreprocess, PreprocessRefusal> {
    use ResourceDeclarationReason::{InvalidType, InvalidValue, MissingField, UnknownField};
    let object = value
        .as_object()
        .ok_or_else(|| refusal("", InvalidType, "preprocess must be an object"))?;
    if let Some(field) = object.keys().find(|field| field.as_str() != "steps") {
        return Err(refusal(
            format!("/{}", crate::text_locate::pointer_token(field)),
            UnknownField,
            format!("preprocess has no member '{field}'"),
        ));
    }
    let steps = object
        .get("steps")
        .ok_or_else(|| refusal("/steps", MissingField, "preprocess lacks steps"))?
        .as_array()
        .ok_or_else(|| refusal("/steps", InvalidType, "preprocess steps must be an array"))?;
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(refusal(
            "/steps",
            InvalidValue,
            format!("preprocess has {} steps, 1 to {MAX_STEPS}", steps.len()),
        ));
    }
    let steps = steps
        .iter()
        .enumerate()
        .map(|(index, step)| parse_step(index, step))
        .collect::<Result<Vec<_>, _>>()?;
    Preprocess::new(steps)
        .map(OcrPreprocess)
        .map_err(|error| refusal(error.pointer(), InvalidValue, error.to_string()))
}

fn parse_step(index: usize, value: &Value) -> Result<PreprocessStep, PreprocessRefusal> {
    use ResourceDeclarationReason::{InvalidType, InvalidValue, MissingField, UnknownField};
    let base = format!("/steps/{index}");
    let object = value
        .as_object()
        .ok_or_else(|| refusal(base.clone(), InvalidType, "a step must be an object"))?;
    let kind_pointer = format!("{base}/kind");
    let name = object
        .get("kind")
        .ok_or_else(|| refusal(kind_pointer.clone(), MissingField, "a step lacks kind"))?
        .as_str()
        .ok_or_else(|| refusal(kind_pointer.clone(), InvalidType, "kind must be a string"))?;
    let kind = PreprocessKind::from_name(name).ok_or_else(|| {
        refusal(
            kind_pointer.clone(),
            InvalidValue,
            format!("unknown step kind '{name}'"),
        )
    })?;
    let members = kind.members();
    if let Some(field) = object
        .keys()
        .find(|field| field.as_str() != "kind" && !members.contains(&field.as_str()))
    {
        return Err(refusal(
            format!("{base}/{}", crate::text_locate::pointer_token(field)),
            UnknownField,
            format!("a {kind} step has no member '{field}'"),
        ));
    }
    if let Some(missing) = members.iter().find(|member| !object.contains_key(**member)) {
        return Err(refusal(
            format!("{base}/{missing}"),
            MissingField,
            format!("a {kind} step lacks {missing}"),
        ));
    }
    let member = |name: &str| (&object[name], format!("{base}/{name}"));
    Ok(match kind {
        PreprocessKind::KeepColors => {
            let (colors, pointer) = member("colors");
            let colors = colors
                .as_array()
                .ok_or_else(|| refusal(pointer.clone(), InvalidType, "colors must be an array"))?
                .iter()
                .enumerate()
                .map(|(position, color)| triple(color, &format!("{pointer}/{position}")))
                .collect::<Result<Vec<_>, _>>()?;
            let (tolerance, pointer) = member("tolerance");
            PreprocessStep::KeepColors {
                colors,
                tolerance: byte(tolerance, &pointer)?,
            }
        }
        PreprocessKind::KeepRange => {
            let (space, pointer) = member("space");
            let name = space
                .as_str()
                .ok_or_else(|| refusal(pointer.clone(), InvalidType, "space must be a string"))?;
            let space = ColorSpace::from_name(name).ok_or_else(|| {
                refusal(
                    pointer.clone(),
                    InvalidValue,
                    format!("space must be rgb or hsv, not '{name}'"),
                )
            })?;
            let (lower, lower_pointer) = member("lower");
            let (upper, upper_pointer) = member("upper");
            PreprocessStep::KeepRange {
                space,
                lower: triple(lower, &lower_pointer)?,
                upper: triple(upper, &upper_pointer)?,
            }
        }
        PreprocessKind::Gray => PreprocessStep::Gray,
        PreprocessKind::Contrast => {
            let (low, low_pointer) = member("low");
            let (high, high_pointer) = member("high");
            PreprocessStep::Contrast {
                low: byte(low, &low_pointer)?,
                high: byte(high, &high_pointer)?,
            }
        }
        PreprocessKind::Threshold => {
            let (value, pointer) = member("value");
            let value = match value.as_str() {
                Some(OTSU) => ThresholdValue::Otsu,
                Some(other) => {
                    return Err(refusal(
                        pointer,
                        InvalidValue,
                        format!("a threshold value is 1 to 255 or \"{OTSU}\", not '{other}'"),
                    ));
                }
                // 0 reaches the chain's own check (`ThresholdZero`).
                None => ThresholdValue::Fixed(byte(value, &pointer)?),
            };
            PreprocessStep::Threshold { value }
        }
        PreprocessKind::Invert => PreprocessStep::Invert,
    })
}

/// An integer 0 to 255: a larger or negative integer is `InvalidValue`, anything else
/// `InvalidType`.
fn byte(value: &Value, pointer: &str) -> Result<u8, PreprocessRefusal> {
    match value.as_u64() {
        Some(number) => u8::try_from(number).map_err(|_| {
            refusal(
                pointer,
                ResourceDeclarationReason::InvalidValue,
                format!("{number} is outside 0 to 255"),
            )
        }),
        None if value.is_i64() => Err(refusal(
            pointer,
            ResourceDeclarationReason::InvalidValue,
            "a negative number is outside 0 to 255",
        )),
        None => Err(refusal(
            pointer,
            ResourceDeclarationReason::InvalidType,
            "an integer 0 to 255 is expected",
        )),
    }
}

/// Three integers 0 to 255; another length is `InvalidValue`.
fn triple(value: &Value, pointer: &str) -> Result<[u8; 3], PreprocessRefusal> {
    let items = value.as_array().ok_or_else(|| {
        refusal(
            pointer,
            ResourceDeclarationReason::InvalidType,
            "three integers are expected",
        )
    })?;
    if items.len() != 3 {
        return Err(refusal(
            pointer,
            ResourceDeclarationReason::InvalidValue,
            format!("three integers are expected, not {}", items.len()),
        ));
    }
    Ok([
        byte(&items[0], &format!("{pointer}/0"))?,
        byte(&items[1], &format!("{pointer}/1"))?,
        byte(&items[2], &format!("{pointer}/2"))?,
    ])
}

fn steps_json(steps: &[PreprocessStep]) -> Value {
    Value::Array(steps.iter().map(step_json).collect())
}

fn step_json(step: &PreprocessStep) -> Value {
    let mut object = Map::new();
    object.insert("kind".to_owned(), Value::from(step.kind().name()));
    let numbers = |values: &[u8; 3]| Value::Array(values.iter().map(|v| Value::from(*v)).collect());
    match step {
        PreprocessStep::KeepColors { colors, tolerance } => {
            object.insert(
                "colors".to_owned(),
                Value::Array(colors.iter().map(numbers).collect()),
            );
            object.insert("tolerance".to_owned(), Value::from(*tolerance));
        }
        PreprocessStep::KeepRange {
            space,
            lower,
            upper,
        } => {
            object.insert("space".to_owned(), Value::from(space.name()));
            object.insert("lower".to_owned(), numbers(lower));
            object.insert("upper".to_owned(), numbers(upper));
        }
        PreprocessStep::Contrast { low, high } => {
            object.insert("low".to_owned(), Value::from(*low));
            object.insert("high".to_owned(), Value::from(*high));
        }
        PreprocessStep::Threshold { value } => {
            let value = match value {
                ThresholdValue::Fixed(threshold) => Value::from(*threshold),
                ThresholdValue::Otsu => Value::from(OTSU),
            };
            object.insert("value".to_owned(), value);
        }
        PreprocessStep::Gray | PreprocessStep::Invert => {}
    }
    Value::Object(object)
}

fn serialize_steps<S: Serializer>(
    preprocess: &OcrPreprocess,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    steps_json(preprocess.0.steps()).serialize(serializer)
}

/// How a chained read's evidence was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrPreprocessScope {
    /// The target processed its own region.
    Target,
    /// The target took its text from a layout's shared read; its evidence is counted over its own
    /// region of the shared read's processed region.
    Shared,
}

/// The evidence of one chained read (design CF v2 §5.7): enough to recompute the processed region
/// and its evidence from the frame with CFL-1's functions. Read-only and Lab output only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OcrPreprocessEvidence {
    /// [`PREPROCESS_ALGORITHM`], `preprocess.v1`.
    pub algorithm: &'static str,
    /// The chain, as declared.
    #[serde(serialize_with = "serialize_steps")]
    pub steps: OcrPreprocess,
    pub scope: OcrPreprocessScope,
    /// The rectangle processed (or, for `shared`, counted), in frame coordinates.
    pub rect: PackRect,
    /// `shared` only: the shared read's processed region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_rect: Option<PackRect>,
    /// The share of values of at least 128 before `invert` over `rect`, in thousandths.
    pub kept_milli: u32,
    /// Whether the chain has `invert`.
    pub inverted: bool,
    /// The threshold used over `rect`; `null` without a threshold step and for Otsu over a region
    /// of one value.
    pub threshold_value: Option<u8>,
}

/// A layout's shared read as the evaluations it serves carry it (design CF v2 §5.6): its
/// rectangle, the detector's box count and limit when the provider gives them, and, with a chain,
/// the evidence over its whole region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OcrSharedReadEvidence {
    pub rect: PackRect,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// With a chain only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_milli: Option<u32>,
    /// With a chain that thresholds only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_value: Option<u8>,
}

/// What one OCR read hands its provider (module doc), and how its blocks come back.
pub(crate) struct OcrHandOver<'s> {
    pixels: Cow<'s, [u8]>,
    width: u32,
    height: u32,
    region: PackRect,
    blocks: OcrProviderBlocks,
    /// The region's origin in the frame, added back onto every block; `None` without a chain.
    origin: Option<PackPoint>,
    evidence: Option<OcrPreprocessEvidence>,
    /// A shared read's processed region, one value per pixel, kept for its slots' evidence.
    processed: Option<Arc<Vec<u8>>>,
}

impl OcrHandOver<'_> {
    /// The provider request of this hand-over.
    pub(crate) fn request<'r>(
        &'r self,
        languages: &'r [String],
        timeout_ms: u64,
        model_ref: &'r str,
        model_sha256: &'r str,
    ) -> OcrProviderRequest<'r> {
        OcrProviderRequest {
            frame: VisionProviderFrame {
                width: self.width,
                height: self.height,
                rgb8_pixels: self.pixels.as_ref(),
            },
            region: self.region,
            languages,
            timeout_ms,
            model_ref,
            model_sha256,
            blocks: self.blocks,
        }
    }

    /// Whether the read runs a chain.
    pub(crate) fn preprocessed(&self) -> bool {
        self.origin.is_some()
    }

    /// The provider's `result` in frame coordinates: each block of a chained read moved back
    /// onto the region's origin, with checked additions; text, confidences and the detector's
    /// count stay as they are.
    pub(crate) fn restore(
        &self,
        mut result: OcrProviderResult,
    ) -> RecognitionPackResult<OcrProviderResult> {
        let Some(origin) = self.origin else {
            return Ok(result);
        };
        for (index, block) in result.blocks.iter_mut().enumerate() {
            match (
                block.rect.x.checked_add(origin.x),
                block.rect.y.checked_add(origin.y),
            ) {
                (Some(x), Some(y)) => {
                    block.rect.x = x;
                    block.rect.y = y;
                }
                _ => {
                    return Err(invalid_provider_response(format!(
                        "OCR block[{index}] rect leaves the i32 range when moved onto the region"
                    ))
                    .with_ppocr_diagnostics(result.ppocr_diagnostics.clone()));
                }
            }
        }
        Ok(result)
    }

    /// The chain's evidence; `None` without a chain.
    pub(crate) fn into_evidence(self) -> Option<OcrPreprocessEvidence> {
        self.evidence
    }

    /// A shared read's evidence and processed region.
    pub(crate) fn into_shared(self) -> (Option<OcrPreprocessEvidence>, Option<Arc<Vec<u8>>>) {
        (self.evidence, self.processed)
    }
}

#[cfg(test)]
thread_local! {
    /// Workflow #428 CFL-2 (design CF v2 §5.8 M1): the chains this thread ran.
    static CHAIN_RUNS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The chains this thread ran so far (design CF v2 §5.8 M1).
#[cfg(test)]
pub(crate) fn chain_runs() -> u32 {
    CHAIN_RUNS.with(std::cell::Cell::get)
}

/// The one instant the logic tests' fake clocks count from, taken once per process, so that a
/// test's clock reads fixed offsets from it and never the time that passed.
#[cfg(test)]
pub(crate) fn test_epoch() -> std::time::Instant {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
}

/// Counts one chain run (design CF v2 §5.8 M1).
#[cfg(test)]
fn count_chain_run() {
    CHAIN_RUNS.with(|runs| runs.set(runs.get() + 1));
}

/// Nothing outside tests.
#[cfg(not(test))]
fn count_chain_run() {}

/// The hand-over of one OCR read of `region` on `scene` (module doc). With a chain the region,
/// already resolved on the frame, is processed once; `keep` keeps the processed region (a shared
/// read's). A chained `Implicit` read of the whole frame is refused: admission refuses its
/// declaration, so this is never reached.
pub(crate) fn hand_over<'s>(
    scene: &'s Scene,
    target_id: &str,
    region: PackRect,
    blocks: OcrProviderBlocks,
    preprocess: Option<&OcrPreprocess>,
    keep: bool,
) -> RecognitionPackResult<OcrHandOver<'s>> {
    let Some(preprocess) = preprocess else {
        return Ok(OcrHandOver {
            pixels: Cow::Borrowed(scene.rgb8_pixels()),
            width: scene.width(),
            height: scene.height(),
            region,
            blocks,
            origin: None,
            evidence: None,
            processed: None,
        });
    };
    let full_frame = region.x == 0
        && region.y == 0
        && u32::try_from(region.width).is_ok_and(|width| width == scene.width())
        && u32::try_from(region.height).is_ok_and(|height| height == scene.height());
    if blocks == OcrProviderBlocks::Implicit && full_frame {
        return Err(RecognitionPackError::fatal(format!(
            "ocr target '{target_id}' declares preprocess on a full-frame region without locate"
        )));
    }
    count_chain_run();
    let output = scene
        .preprocess_region(region.into(), preprocess.chain())
        .map_err(|error| primitive_error(target_id, error))?;
    let (width, height) = match (u32::try_from(region.width), u32::try_from(region.height)) {
        (Ok(width), Ok(height)) => (width, height),
        _ => {
            return Err(RecognitionPackError::fatal(format!(
                "ocr target '{target_id}' preprocess region has a negative size"
            )));
        }
    };
    let evidence = OcrPreprocessEvidence {
        algorithm: PREPROCESS_ALGORITHM,
        steps: preprocess.clone(),
        scope: OcrPreprocessScope::Target,
        rect: region,
        source_rect: None,
        kept_milli: output.evidence.kept_milli,
        inverted: output.evidence.inverted,
        threshold_value: output.evidence.threshold_value,
    };
    let gray = output.image.into_raw();
    let mut pixels = Vec::with_capacity(gray.len() * 3);
    for value in &gray {
        pixels.extend_from_slice(&[*value, *value, *value]);
    }
    let processed = keep.then(|| Arc::new(gray));
    Ok(OcrHandOver {
        pixels: Cow::Owned(pixels),
        width,
        height,
        region: PackRect {
            x: 0,
            y: 0,
            width: region.width,
            height: region.height,
        },
        blocks: match blocks {
            OcrProviderBlocks::Implicit => OcrProviderBlocks::Region,
            explicit => explicit,
        },
        origin: Some(PackPoint {
            x: region.x,
            y: region.y,
        }),
        evidence: Some(evidence),
        processed,
    })
}

/// The evidence of a target that takes its text from a shared read (design CF v2 §5.6): the
/// shared read's chain over the target's own `region`, counted in the shared read's processed
/// region (`source`, at `source_rect`) without running the chain again. The chain works pixel by
/// pixel (admission refuses `otsu` there), so this equals the target's own processing.
pub(crate) fn shared_evidence(
    shared: &OcrPreprocessEvidence,
    source: &[u8],
    source_rect: PackRect,
    region: PackRect,
) -> RecognitionPackResult<OcrPreprocessEvidence> {
    let offset = |value: i32, origin: i32| {
        value
            .checked_sub(origin)
            .and_then(|offset| usize::try_from(offset).ok())
    };
    let size = |value: i32| usize::try_from(value).ok();
    let (Some(left), Some(top), Some(width), Some(height), Some(stride)) = (
        offset(region.x, source_rect.x),
        offset(region.y, source_rect.y),
        size(region.width),
        size(region.height),
        size(source_rect.width),
    ) else {
        return Err(RecognitionPackError::fatal(
            "a shared read's target region lies outside its region",
        ));
    };
    let mut kept = 0_u64;
    for row in top..top + height {
        let start = row * stride + left;
        let values = source.get(start..start + width).ok_or_else(|| {
            RecognitionPackError::fatal("a shared read's target region lies outside its region")
        })?;
        // A value of at least 128 before `invert` is one below 128 after it.
        kept += values
            .iter()
            .filter(|value| (**value >= 128) != shared.inverted)
            .count() as u64;
    }
    let pixels = (width as u64) * (height as u64);
    let kept_milli = u32::try_from(kept * 1000 / pixels.max(1))
        .map_err(|_| RecognitionPackError::fatal("the kept share leaves the u32 range"))?;
    Ok(OcrPreprocessEvidence {
        algorithm: shared.algorithm,
        steps: shared.steps.clone(),
        scope: OcrPreprocessScope::Shared,
        rect: region,
        source_rect: Some(source_rect),
        kept_milli,
        inverted: shared.inverted,
        // Without `otsu` the threshold is the declared one, the same over every part.
        threshold_value: shared.threshold_value,
    })
}

impl<'a> SceneEvaluation<'a> {
    /// The hand-over of one OCR read here (module doc). After a chain ran, the sample deadline,
    /// when this context has one, is checked again as before the read: the time left on this
    /// context's clock must hold the provider's `timeout_ms`, or the read fails with the existing
    /// error and the provider is not called (design CF v2 §5.8).
    pub(crate) fn ocr_hand_over(
        &self,
        target_id: &str,
        region: PackRect,
        blocks: OcrProviderBlocks,
        preprocess: Option<&OcrPreprocess>,
        timeout_ms: u64,
    ) -> RecognitionPackResult<OcrHandOver<'a>> {
        let scene: &'a Scene = self.scene;
        let hand_over = hand_over(scene, target_id, region, blocks, preprocess, false)?;
        if hand_over.preprocessed() {
            self.recheck_sample_budget(timeout_ms)?;
        }
        Ok(hand_over)
    }

    /// Fails with the existing `recognition sample budget insufficient` error when this context's
    /// sample deadline leaves less than `provider_ms` on its clock.
    pub(crate) fn recheck_sample_budget(&self, provider_ms: u64) -> RecognitionPackResult<()> {
        if self.sample_deadline.is_some_and(|deadline| {
            std::time::Duration::from_millis(provider_ms)
                > deadline.saturating_duration_since((self.search_clock)())
        }) {
            return Err(RecognitionPackError::fatal(
                "recognition sample budget insufficient before backend evaluation",
            ));
        }
        Ok(())
    }
}

/// Workflow #428 CFL-2: the declaration rules of `preprocess` that read the pack beyond the
/// declaration itself (design CF v2 §5.12): an OCR target whose region is the whole frame
/// (`full_frame`, or a `rect` or `template_relative` region the size of `coordinate_space`)
/// declares `locate` with its chain, since its implicit read would drop boxes beyond the folder's
/// `max_boxes` without a count. The first refused declaration is returned with its pointer.
pub(crate) fn validate_preprocess_declarations(
    pack: &RecognitionPack,
) -> Result<(), crate::text_locate::LocateRefusal> {
    let space = pack
        .coordinate_space
        .map(|space| (i64::from(space.width), i64::from(space.height)));
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::Ocr(target) = target else {
            continue;
        };
        if target.preprocess.is_none() || target.locate.is_some() {
            continue;
        }
        let size = match &target.region {
            PackRegion::Keyword(value) if value == "full_frame" => space,
            PackRegion::Keyword(_) => None,
            PackRegion::Rect(rect) => Some((i64::from(rect.width), i64::from(rect.height))),
            PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                width,
                height,
                ..
            }) => Some((i64::from(*width), i64::from(*height))),
        };
        let full_frame = matches!(&target.region, PackRegion::Keyword(value) if value == "full_frame")
            || (size.is_some() && size == space);
        if full_frame {
            crate::text_locate::refuse(
                format!("/targets/{index}/preprocess"),
                ResourceDeclarationReason::InvalidValue,
                "an OCR target over the whole frame declares preprocess only with locate",
            )?;
        }
    }
    Ok(())
}

/// The wire check of a `preprocess` member at `pointer`: its declaration parsed, a refusal
/// reported at its own pointer. With `per_pixel` (a shared read's), a `threshold` of `"otsu"` is
/// refused too (`InvalidValue` at its `value`).
pub(crate) fn validate_preprocess_wire(
    value: &Value,
    pointer: &str,
    per_pixel: bool,
) -> RecognitionPackResult<()> {
    let refused = |refusal: PreprocessRefusal| {
        RecognitionPackError::fatal(format!("{pointer}: {}", refusal.explanation))
            .at_declaration(format!("{pointer}{}", refusal.pointer), refusal.reason)
    };
    let preprocess = parse_preprocess(value).map_err(refused)?;
    if per_pixel && let Some(step) = preprocess.otsu_step() {
        return Err(refused(shared_otsu_refusal(step)));
    }
    Ok(())
}

/// A shared read's `otsu` step `step`, refused: Otsu's threshold reads the whole region, so a
/// slot's part of the shared result would differ from what the slot gives on its own.
pub fn shared_otsu_refusal(step: usize) -> PreprocessRefusal {
    refusal(
        format!("/steps/{step}/value"),
        ResourceDeclarationReason::InvalidValue,
        "a shared read takes only pixel-by-pixel chains; threshold \"otsu\" reads the whole region",
    )
}
