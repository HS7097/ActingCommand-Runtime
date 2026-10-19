// SPDX-License-Identifier: AGPL-3.0-only

//! The five mark families (template, color, color digest, OCR, check), their shape rules
//! and the mark-time self-test on every live frame of the step.

use super::frames::{LoadedFrame, crop_png, rect_contains, rect_inside};
use super::model::{
    LAB_RECORDING_DEFAULT_SEARCH_MARGIN, MarkCrop, MarkFamily, MarkSpec, OcrSpec, RecordRect,
    RecordSize, RecordedMark, RecordingDefaults, SelfTest, SelfTestMargin, SelfTestStatus,
};
use super::store::{blocked, hex_sha256, invalid};
use crate::LabOcr;
use crate::codes::LabCode;
use actingcommand_contract::LabResult;
use actingcommand_recognition::color_digest::{
    self, ColorDigest, ColorDigestGrid, ColorDigestThresholds,
};
use actingcommand_recognition::{MatchMetric, Rect};
use actingcommand_recognition_pack::{
    AssetResolver, FsAssetResolver, RecognitionEvaluator, RecognitionPackError,
    RecognitionPackResult, TargetEvaluation, VisionProvider, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) const MAX_MARK_ID_BYTES: usize = 128;
const CHECK_MEMBERS: std::ops::RangeInclusive<usize> = 2..=8;

/// One live frame of the step (primary, samples, or the transition's frames).
pub(crate) struct EvalFrame<'a> {
    pub(crate) id: &'a str,
    pub(crate) frame: &'a LoadedFrame,
}

/// A mark the batch refuses; collected into one `record_mark_rejected`.
pub(crate) struct MarkRejection {
    pub(crate) id: String,
    pub(crate) reason: String,
    pub(crate) detail: Value,
}

impl MarkRejection {
    pub(crate) fn new(id: &str, reason: &str, detail: Value) -> Self {
        Self {
            id: id.to_string(),
            reason: reason.to_string(),
            detail,
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut value = json!({"id": self.id, "reason": self.reason});
        if let (Some(object), Some(detail)) = (value.as_object_mut(), self.detail.as_object()) {
            for (key, entry) in detail {
                object.insert(key.clone(), entry.clone());
            }
        }
        value
    }
}

pub(crate) struct PreparedMark {
    pub(crate) mark: RecordedMark,
    pub(crate) crop_png: Option<Vec<u8>>,
}

pub(crate) fn validate_mark_id(id: &str) -> LabResult<()> {
    if id.is_empty()
        || id.len() > MAX_MARK_ID_BYTES
        || id.trim() != id
        || id.chars().any(char::is_control)
    {
        return Err(invalid(
            "validation_failed",
            format!(
                "mark id '{id}' must be 1..={MAX_MARK_ID_BYTES} bytes without surrounding \
                 whitespace or control characters"
            ),
        ));
    }
    if id.starts_with("page/") {
        return Err(blocked(
            "record_mark_id_reserved",
            format!("mark id '{id}' uses the reserved page/ prefix of anchor targets"),
        ));
    }
    Ok(())
}

/// `assets/<id lowercased, characters outside [a-z0-9_.-] replaced by _>.png`.
pub(crate) fn template_asset_name(id: &str) -> String {
    let name = id
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("assets/{name}.png")
}

pub(crate) fn match_metric(name: &str) -> LabResult<MatchMetric> {
    match name {
        "ccoeff_normed" => Ok(MatchMetric::CorrelationCoefficientNormalized),
        "ccorr_normed" => Ok(MatchMetric::CrossCorrelationNormalized),
        other => Err(invalid(
            "validation_failed",
            format!("unsupported match metric '{other}', expected ccoeff_normed or ccorr_normed"),
        )),
    }
}

fn recognition_rect(rect: RecordRect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn reject_fields(spec: &MarkSpec, fields: &[(&str, bool)]) -> LabResult<()> {
    let present = fields
        .iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    if present.is_empty() {
        return Ok(());
    }
    Err(invalid(
        "validation_failed",
        format!(
            "mark '{}' of family {} does not accept: {}",
            spec.id,
            spec.family.as_str(),
            present.join(", ")
        ),
    ))
}

fn not_template(spec: &MarkSpec) -> [(&'static str, bool); 2] {
    [
        ("search", spec.search.is_some()),
        ("threshold", spec.threshold.is_some()),
    ]
}

fn not_color(spec: &MarkSpec) -> [(&'static str, bool); 1] {
    [("max_distance", spec.max_distance.is_some())]
}

fn not_digest(spec: &MarkSpec) -> [(&'static str, bool); 5] {
    [
        ("columns", spec.columns.is_some()),
        ("rows", spec.rows.is_some()),
        ("max_mean_milli", spec.max_mean_milli.is_some()),
        ("max_cell", spec.max_cell.is_some()),
        ("exclude_cells", spec.exclude_cells.is_some()),
    ]
}

fn not_ocr(spec: &MarkSpec) -> [(&'static str, bool); 8] {
    [
        ("languages", spec.languages.is_some()),
        ("timeout_ms", spec.timeout_ms.is_some()),
        ("match_mode", spec.match_mode.is_some()),
        ("expected", spec.expected.is_some()),
        ("case_sensitive", spec.case_sensitive.is_some()),
        ("minimum_confidence", spec.minimum_confidence.is_some()),
        ("model_ref", spec.model_ref.is_some()),
        ("model_sha256", spec.model_sha256.is_some()),
    ]
}

fn not_check(spec: &MarkSpec) -> [(&'static str, bool); 2] {
    [
        ("all_of", spec.all_of.is_some()),
        ("any_of", spec.any_of.is_some()),
    ]
}

/// Workflow #396 REC-4: only a template mark declares factors, a mask or uniqueness.
fn not_exact_template(spec: &MarkSpec) -> [(&'static str, bool); 3] {
    [
        ("scales", spec.scales.is_some()),
        ("mask", spec.mask.is_some()),
        ("unique", spec.unique.is_some()),
    ]
}

fn empty_mark(spec: &MarkSpec, step: u32, transition_of: Option<u32>, now: u64) -> RecordedMark {
    RecordedMark {
        id: spec.id.clone(),
        family: spec.family,
        step,
        transition_of,
        region: spec.region,
        search: None,
        threshold: None,
        metric: None,
        crop: None,
        expected: None,
        max_distance: None,
        columns: None,
        rows: None,
        cells: None,
        exclude_cells: None,
        max_mean_milli: None,
        max_cell: None,
        ocr: None,
        all_of: None,
        any_of: None,
        scales: None,
        mask: None,
        unique: None,
        self_test: SelfTest {
            status: SelfTestStatus::NotEvaluated,
            reason: None,
            frames: 0,
            single_sample: false,
            min_score: None,
            margin: None,
            matched_rect: None,
            location_ambiguous: None,
            max_observed_distance: None,
            mean_milli: None,
            max_cell: None,
            members: None,
            matched_scales_milli: None,
        },
        created_at_unix_ms: now,
    }
}

/// Shape rules (exit 2) and the values taken from the primary frame. Region problems are
/// rejections of the batch, not shape errors.
/// Workflow #390 LAB-2: the search area of a template mark that declares none: its crop grown by
/// `LAB_RECORDING_DEFAULT_SEARCH_MARGIN` pixels on each side and clamped to the frame, so that a
/// control that moved a little still matches; a declared `search` replaces it.
pub(crate) fn default_search(region: RecordRect, size: RecordSize) -> RecordRect {
    let margin = i64::from(LAB_RECORDING_DEFAULT_SEARCH_MARGIN);
    let left = (i64::from(region.x) - margin).max(0);
    let top = (i64::from(region.y) - margin).max(0);
    let right = (i64::from(region.x) + i64::from(region.width) + margin).min(i64::from(size.width));
    let bottom =
        (i64::from(region.y) + i64::from(region.height) + margin).min(i64::from(size.height));
    let field = |value: i64| i32::try_from(value).unwrap_or(i32::MAX);
    RecordRect {
        x: field(left),
        y: field(top),
        width: field(right - left),
        height: field(bottom - top),
    }
}

pub(crate) fn prepare_mark(
    spec: &MarkSpec,
    primary: &LoadedFrame,
    defaults: &RecordingDefaults,
    step: u32,
    transition_of: Option<u32>,
    now: u64,
) -> LabResult<Result<PreparedMark, MarkRejection>> {
    validate_mark_id(&spec.id)?;
    let mut mark = empty_mark(spec, step, transition_of, now);
    if spec.family == MarkFamily::Check {
        reject_fields(
            spec,
            &[
                ("region", spec.region.is_some()),
                ("search", spec.search.is_some()),
                ("threshold", spec.threshold.is_some()),
                ("max_distance", spec.max_distance.is_some()),
            ],
        )?;
        reject_fields(spec, &not_digest(spec))?;
        reject_fields(spec, &not_ocr(spec))?;
        reject_fields(spec, &not_exact_template(spec))?;
        let members = match (&spec.all_of, &spec.any_of) {
            (Some(members), None) => {
                mark.all_of = Some(members.clone());
                members
            }
            (None, Some(members)) => {
                mark.any_of = Some(members.clone());
                members
            }
            _ => {
                return Err(invalid(
                    "validation_failed",
                    format!("check '{}' needs exactly one of all_of and any_of", spec.id),
                ));
            }
        };
        let distinct = members.iter().collect::<std::collections::BTreeSet<_>>();
        if !CHECK_MEMBERS.contains(&members.len()) || distinct.len() != members.len() {
            return Err(invalid(
                "validation_failed",
                format!("check '{}' needs 2..=8 distinct members", spec.id),
            ));
        }
        return Ok(Ok(PreparedMark {
            mark,
            crop_png: None,
        }));
    }
    let region = spec.region.ok_or_else(|| {
        invalid(
            "validation_failed",
            format!(
                "mark '{}' of family {} needs a region",
                spec.id,
                spec.family.as_str()
            ),
        )
    })?;
    reject_fields(spec, &not_check(spec))?;
    if !rect_inside(region, primary.size()) {
        return Ok(Err(MarkRejection::new(
            &spec.id,
            "region_outside_frame",
            json!({"region": region, "frame_size": primary.size()}),
        )));
    }
    match spec.family {
        MarkFamily::Template => {
            reject_fields(spec, &not_color(spec))?;
            reject_fields(spec, &not_digest(spec))?;
            reject_fields(spec, &not_ocr(spec))?;
            if let Some(threshold) = spec.threshold
                && (!threshold.is_finite() || !(0.0..=1.0).contains(&threshold))
            {
                return Err(invalid(
                    "validation_failed",
                    format!("template '{}' threshold must be in 0..=1", spec.id),
                ));
            }
            // Workflow #390 LAB-2: without a declared search area, the crop with a margin.
            let search = spec
                .search
                .unwrap_or_else(|| default_search(region, primary.size()));
            if !rect_inside(search, primary.size()) || !rect_contains(search, region) {
                return Ok(Err(MarkRejection::new(
                    &spec.id,
                    "search_invalid",
                    json!({"region": region, "search": search, "frame_size": primary.size()}),
                )));
            }
            let png = crop_png(primary, region)?;
            let sha256 = hex_sha256(&png);
            mark.search = Some(search);
            mark.threshold = spec.threshold;
            mark.metric = Some(defaults.match_metric.clone());
            // Workflow #396 REC-4: the self-test checks these as the Runtime admits them.
            mark.scales = spec.scales.clone();
            mark.mask = spec.mask;
            mark.unique = spec.unique;
            mark.crop = Some(MarkCrop {
                path: String::new(),
                sha256,
                width: region.width as u32,
                height: region.height as u32,
                asset: template_asset_name(&spec.id),
            });
            Ok(Ok(PreparedMark {
                mark,
                crop_png: Some(png),
            }))
        }
        MarkFamily::Color => {
            reject_fields(spec, &not_template(spec))?;
            reject_fields(spec, &not_digest(spec))?;
            reject_fields(spec, &not_ocr(spec))?;
            reject_fields(spec, &not_exact_template(spec))?;
            if let Some(distance) = spec.max_distance
                && (!distance.is_finite() || distance < 0.0)
            {
                return Err(invalid(
                    "validation_failed",
                    format!("color '{}' max_distance must be finite and >= 0", spec.id),
                ));
            }
            let measured = primary
                .scene
                .compare_color(recognition_rect(region), [0, 0, 0])
                .map_err(|error| {
                    invalid(
                        "validation_failed",
                        format!("color '{}' cannot be sampled: {error}", spec.id),
                    )
                })?;
            mark.expected = Some(measured.mean);
            mark.max_distance = spec.max_distance;
            Ok(Ok(PreparedMark {
                mark,
                crop_png: None,
            }))
        }
        MarkFamily::ColorDigest => {
            reject_fields(spec, &not_template(spec))?;
            reject_fields(spec, &not_color(spec))?;
            reject_fields(spec, &not_ocr(spec))?;
            reject_fields(spec, &not_exact_template(spec))?;
            let (Some(columns), Some(rows), Some(max_mean_milli)) =
                (spec.columns, spec.rows, spec.max_mean_milli)
            else {
                return Err(invalid(
                    "validation_failed",
                    format!(
                        "color_digest '{}' needs columns, rows and max_mean_milli",
                        spec.id
                    ),
                ));
            };
            let grid = ColorDigestGrid::new(columns, rows).map_err(|error| {
                invalid(
                    "validation_failed",
                    format!("color_digest '{}' grid: {error}", spec.id),
                )
            })?;
            let exclude = spec.exclude_cells.clone().unwrap_or_default();
            color_digest::validate_exclude_cells(grid, &exclude).map_err(|error| {
                invalid(
                    "validation_failed",
                    format!("color_digest '{}' exclude_cells: {error}", spec.id),
                )
            })?;
            let digest = ColorDigest::compute(&primary.scene, recognition_rect(region), grid)
                .map_err(|error| {
                    invalid(
                        "validation_failed",
                        format!(
                            "color_digest '{}' grid does not fit the region: {error}",
                            spec.id
                        ),
                    )
                })?;
            mark.columns = Some(columns);
            mark.rows = Some(rows);
            mark.cells = Some(digest.to_hex());
            mark.exclude_cells = spec.exclude_cells.clone();
            mark.max_mean_milli = Some(max_mean_milli);
            mark.max_cell = spec.max_cell;
            Ok(Ok(PreparedMark {
                mark,
                crop_png: None,
            }))
        }
        MarkFamily::Ocr => {
            reject_fields(spec, &not_template(spec))?;
            reject_fields(spec, &not_color(spec))?;
            reject_fields(spec, &not_digest(spec))?;
            reject_fields(spec, &not_exact_template(spec))?;
            mark.ocr = Some(ocr_spec(spec)?);
            Ok(Ok(PreparedMark {
                mark,
                crop_png: None,
            }))
        }
        MarkFamily::Check => unreachable!("check handled above"),
    }
}

fn ocr_spec(spec: &MarkSpec) -> LabResult<OcrSpec> {
    let missing = |field: &str| {
        invalid(
            "validation_failed",
            format!("ocr '{}' requires {field}", spec.id),
        )
    };
    let ocr = OcrSpec {
        languages: spec.languages.clone().ok_or_else(|| missing("languages"))?,
        timeout_ms: spec.timeout_ms.ok_or_else(|| missing("timeout_ms"))?,
        match_mode: spec
            .match_mode
            .clone()
            .ok_or_else(|| missing("match_mode"))?,
        expected: spec.expected.clone().ok_or_else(|| missing("expected"))?,
        case_sensitive: spec
            .case_sensitive
            .ok_or_else(|| missing("case_sensitive"))?,
        minimum_confidence: spec
            .minimum_confidence
            .ok_or_else(|| missing("minimum_confidence"))?,
        model_ref: spec.model_ref.clone().ok_or_else(|| missing("model_ref"))?,
        model_sha256: spec
            .model_sha256
            .clone()
            .ok_or_else(|| missing("model_sha256"))?,
    };
    let sha_valid = ocr.model_sha256.len() == 64
        && ocr
            .model_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if ocr.languages.is_empty()
        || ocr
            .languages
            .iter()
            .any(|language| language.trim().is_empty())
        || ocr.expected.is_empty()
        || ocr.timeout_ms == 0
        || !matches!(ocr.match_mode.as_str(), "exact" | "contains")
        || !ocr.minimum_confidence.is_finite()
        || !(0.0..=1.0).contains(&ocr.minimum_confidence)
        || ocr.model_ref.trim().is_empty()
        || !sha_valid
    {
        return Err(invalid(
            "validation_failed",
            format!(
                "ocr '{}' needs non-empty languages and expected, timeout_ms > 0, match_mode \
                 exact or contains, minimum_confidence in 0..=1, a model_ref and a lowercase \
                 64-hex model_sha256",
                spec.id
            ),
        ));
    }
    Ok(ocr)
}

fn self_test(status: SelfTestStatus, frames: u32) -> SelfTest {
    SelfTest {
        status,
        reason: None,
        frames,
        single_sample: frames == 1,
        min_score: None,
        margin: None,
        matched_rect: None,
        location_ambiguous: None,
        max_observed_distance: None,
        mean_milli: None,
        max_cell: None,
        members: None,
        matched_scales_milli: None,
    }
}

/// Self-test of a template, color, color-digest or OCR mark on every live frame. The first
/// failing frame is returned for the rejection details.
pub(crate) fn evaluate_mark(
    mark: &RecordedMark,
    template_png: Option<&[u8]>,
    frames: &[EvalFrame<'_>],
    defaults: &RecordingDefaults,
    ocr: &LabOcr,
) -> LabResult<(SelfTest, Option<Value>)> {
    let count = u32::try_from(frames.len()).unwrap_or(u32::MAX);
    match mark.family {
        MarkFamily::Ocr => match ocr.ocr_provider() {
            Some(provider) => evaluate_ocr(mark, frames, count, provider),
            // Workflow #396 LAB-3: without an OCR model the mark is not evaluated, under the
            // registered reason; not evaluated is never reported as passed.
            None => {
                let mut test = self_test(SelfTestStatus::NotEvaluated, count);
                test.reason = Some(LabCode::LabOcrUnavailable.as_str().to_string());
                Ok((test, None))
            }
        },
        MarkFamily::Check => Err(invalid(
            "validation_failed",
            "a check mark is derived from its members",
        )),
        MarkFamily::Template => evaluate_template(mark, template_png, frames, defaults, count),
        MarkFamily::Color => evaluate_color(mark, frames, defaults, count),
        MarkFamily::ColorDigest => evaluate_digest(mark, frames, count),
    }
}

fn region_of(mark: &RecordedMark) -> LabResult<RecordRect> {
    mark.region.ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("mark '{}' has no region", mark.id),
        )
    })
}

fn evaluate_template(
    mark: &RecordedMark,
    template_png: Option<&[u8]>,
    frames: &[EvalFrame<'_>],
    defaults: &RecordingDefaults,
    count: u32,
) -> LabResult<(SelfTest, Option<Value>)> {
    let region = region_of(mark)?;
    let search = mark.search.unwrap_or(region);
    let png = template_png.ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("template '{}' has no crop", mark.id),
        )
    })?;
    if mark.declares_exact_template() {
        return evaluate_declared_template(mark, png, frames, defaults, count);
    }
    let metric_name = mark
        .metric
        .clone()
        .unwrap_or_else(|| defaults.match_metric.clone());
    let metric = match_metric(&metric_name)?;
    let threshold = mark.threshold.unwrap_or(defaults.template_threshold) as f32;
    let mut test = self_test(SelfTestStatus::Passed, count);
    let mut min_score: Option<f32> = None;
    let mut ambiguous = false;
    let mut failure = None;
    for (index, eval) in frames.iter().enumerate() {
        let matched = match eval.frame.scene.match_template_with_metric(
            png,
            Some(recognition_rect(search)),
            metric,
        ) {
            Ok(matched) => matched,
            Err(error) => {
                test.status = SelfTestStatus::Failed;
                test.reason = Some("self_test_error".to_string());
                failure.get_or_insert(json!({
                    "frame_id": eval.id,
                    "error": error.to_string(),
                    "threshold": threshold
                }));
                continue;
            }
        };
        if index == 0 {
            test.matched_rect = Some(RecordRect {
                x: matched.x,
                y: matched.y,
                width: matched.width,
                height: matched.height,
            });
        }
        ambiguous |= matched.x != region.x || matched.y != region.y;
        min_score = Some(min_score.map_or(matched.score, |score| score.min(matched.score)));
        if matched.score < threshold {
            test.status = SelfTestStatus::Failed;
            test.reason = Some("self_test_failed".to_string());
            failure.get_or_insert(json!({
                "frame_id": eval.id,
                "score": matched.score,
                "threshold": threshold
            }));
        }
    }
    if let Some(score) = min_score {
        test.min_score = Some(f64::from(score));
        test.margin = Some(SelfTestMargin::Value(
            f64::from(score) - f64::from(threshold),
        ));
    }
    test.location_ambiguous = Some(ambiguous);
    Ok((test, failure))
}

/// The asset path of a template mark's crop in the pack its evaluator loads.
const MARK_TEMPLATE_ASSET: &str = "template.png";

/// The crop of one template mark, the only asset of the pack its evaluator loads.
#[derive(Debug)]
struct MarkTemplateAsset(Vec<u8>);

impl AssetResolver for MarkTemplateAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == MARK_TEMPLATE_ASSET {
            return Ok(self.0.clone());
        }
        Err(RecognitionPackError::fatal(format!(
            "a template mark's pack has no asset '{path}'"
        )))
    }
}

/// Workflow #396 REC-4: the recognition evaluator of the template target the package declares
/// for `mark` (its search area, threshold and metric, and its factors, mask and margin of
/// uniqueness), over frames of `size`. Loading it applies the Runtime's rules for these
/// declarations; a declaration they refuse is `validation_failed`.
pub(crate) fn template_evaluator(
    mark: &RecordedMark,
    template_png: &[u8],
    defaults: &RecordingDefaults,
    size: RecordSize,
) -> LabResult<RecognitionEvaluator> {
    let region = region_of(mark)?;
    let search = mark.search.unwrap_or(region);
    let metric = mark
        .metric
        .clone()
        .unwrap_or_else(|| defaults.match_metric.clone());
    match_metric(&metric)?;
    let mut target = json!({
        "type": "template",
        "id": mark.id,
        "template_path": MARK_TEMPLATE_ASSET,
        "region": {"x": search.x, "y": search.y, "width": search.width, "height": search.height}
    });
    if let Some(threshold) = mark.threshold {
        target["threshold"] = json!(threshold);
    }
    if let Some(scales) = &mark.scales {
        target["scales"] = json!(scales);
    }
    if let Some(mask) = &mark.mask {
        target["mask"] = json!(mask);
    }
    if let Some(unique) = &mark.unique {
        target["unique"] = json!(unique);
    }
    let pack = json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": size.width, "height": size.height},
        "defaults": {"template_threshold": defaults.template_threshold, "match_metric": metric},
        "targets": [target]
    });
    load_pack_from_json_str(&pack.to_string())
        .and_then(|pack| {
            RecognitionEvaluator::with_asset_resolver(
                pack,
                Arc::new(MarkTemplateAsset(template_png.to_vec())),
            )
        })
        // The rules a package's admission applies with the template bytes (REC-7's mask minimum
        // and threshold floor, REC-5's resampled sizes), so the mark is refused when it is
        // recorded rather than at `record stop`.
        .and_then(|evaluator| evaluator.admit_template_assets().map(|()| evaluator))
        .map_err(|error| {
            invalid(
                "validation_failed",
                format!("template '{}' cannot be evaluated: {error}", mark.id),
            )
        })
}

/// Workflow #396 REC-4: a template mark evaluated on one frame as the Runtime evaluates the
/// target the package declares for it; an evaluation error keeps its text.
pub(crate) struct FrameTemplateEvaluation<'a> {
    pub(crate) frame_id: &'a str,
    pub(crate) size: RecordSize,
    pub(crate) evaluation: Result<TargetEvaluation, String>,
}

/// Workflow #396 REC-4: `mark` evaluated on every frame with [`template_evaluator`].
pub(crate) fn template_frame_evaluations<'a>(
    mark: &RecordedMark,
    template_png: &[u8],
    frames: &[EvalFrame<'a>],
    defaults: &RecordingDefaults,
) -> LabResult<Vec<FrameTemplateEvaluation<'a>>> {
    let Some(first) = frames.first() else {
        return Ok(Vec::new());
    };
    let evaluator = template_evaluator(mark, template_png, defaults, first.frame.size())?;
    Ok(frames
        .iter()
        .map(|eval| FrameTemplateEvaluation {
            frame_id: eval.id,
            size: eval.frame.size(),
            evaluation: evaluator
                .evaluate_target(&eval.frame.scene, &mark.id)
                .map_err(|error| error.to_string()),
        })
        .collect())
}

/// Workflow #396 REC-4: the self-test of a template mark that declares factors (REC-5), a mask
/// (REC-7) or uniqueness (REC-2), with the Runtime's evaluation of its target on every frame.
/// Every frame must pass; a second place within the margin fails the frame and the failure
/// names it (`uniqueness`). `matched_scales_milli` lists the factors it passed at.
fn evaluate_declared_template(
    mark: &RecordedMark,
    template_png: &[u8],
    frames: &[EvalFrame<'_>],
    defaults: &RecordingDefaults,
    count: u32,
) -> LabResult<(SelfTest, Option<Value>)> {
    let region = region_of(mark)?;
    let threshold = mark.threshold.unwrap_or(defaults.template_threshold) as f32;
    let mut test = self_test(SelfTestStatus::Passed, count);
    let mut min_score: Option<f32> = None;
    let mut ambiguous = false;
    let mut failure = None;
    let mut matched = std::collections::BTreeSet::new();
    let evaluations = template_frame_evaluations(mark, template_png, frames, defaults)?;
    for (index, frame) in evaluations.iter().enumerate() {
        let evaluation = match &frame.evaluation {
            Ok(evaluation) => evaluation,
            Err(error) => {
                test.status = SelfTestStatus::Failed;
                test.reason = Some("self_test_error".to_string());
                failure.get_or_insert(json!({
                    "frame_id": frame.frame_id,
                    "error": error,
                    "threshold": threshold
                }));
                continue;
            }
        };
        let Some(template) = evaluation.template else {
            test.status = SelfTestStatus::Failed;
            test.reason = Some("self_test_error".to_string());
            failure.get_or_insert(json!({
                "frame_id": frame.frame_id,
                "threshold": threshold
            }));
            continue;
        };
        if index == 0 {
            test.matched_rect = Some(RecordRect {
                x: template.x,
                y: template.y,
                width: template.width,
                height: template.height,
            });
        }
        ambiguous |= template.x != region.x || template.y != region.y;
        min_score = Some(min_score.map_or(template.score, |score| score.min(template.score)));
        if evaluation.passed {
            matched.extend(template.scale_milli);
            continue;
        }
        test.status = SelfTestStatus::Failed;
        test.reason = Some("self_test_failed".to_string());
        let mut detail = json!({
            "frame_id": frame.frame_id,
            "score": template.score,
            "threshold": template.threshold
        });
        if let Some(scale_milli) = template.scale_milli {
            detail["scale_milli"] = json!(scale_milli);
        }
        // REC-2: the second place within the margin, so that the author can set the margin.
        if let Some(uniqueness) = &evaluation.uniqueness {
            detail["uniqueness"] = json!(uniqueness);
        }
        failure.get_or_insert(detail);
    }
    if let Some(score) = min_score {
        test.min_score = Some(f64::from(score));
        test.margin = Some(SelfTestMargin::Value(
            f64::from(score) - f64::from(threshold),
        ));
    }
    test.location_ambiguous = Some(ambiguous);
    if let Some(scales) = &mark.scales {
        test.matched_scales_milli = Some(
            scales
                .factors_milli
                .iter()
                .copied()
                .filter(|factor| matched.contains(factor))
                .collect(),
        );
    }
    Ok((test, failure))
}

fn evaluate_color(
    mark: &RecordedMark,
    frames: &[EvalFrame<'_>],
    defaults: &RecordingDefaults,
    count: u32,
) -> LabResult<(SelfTest, Option<Value>)> {
    let region = region_of(mark)?;
    let expected = mark.expected.ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("color '{}' has no expected color", mark.id),
        )
    })?;
    let limit = mark
        .max_distance
        .unwrap_or(f64::from(defaults.color_max_distance));
    let mut test = self_test(SelfTestStatus::Passed, count);
    let mut worst = 0.0_f64;
    let mut failure = None;
    for eval in frames {
        let measured = eval
            .frame
            .scene
            .compare_color(recognition_rect(region), expected)
            .map_err(|error| {
                invalid(
                    "validation_failed",
                    format!("color '{}' cannot be measured: {error}", mark.id),
                )
            })?;
        let distance = f64::from(measured.distance);
        worst = worst.max(distance);
        if distance > limit {
            test.status = SelfTestStatus::Failed;
            test.reason = Some("self_test_failed".to_string());
            failure.get_or_insert(json!({
                "frame_id": eval.id,
                "distance": distance,
                "max_distance": limit,
                "measured": measured.mean,
                "expected": expected
            }));
        }
    }
    test.max_observed_distance = Some(worst);
    test.margin = Some(SelfTestMargin::Value(limit - worst));
    Ok((test, failure))
}

fn evaluate_digest(
    mark: &RecordedMark,
    frames: &[EvalFrame<'_>],
    count: u32,
) -> LabResult<(SelfTest, Option<Value>)> {
    let region = region_of(mark)?;
    let shape_error = |message: String| invalid("validation_failed", message);
    let (Some(columns), Some(rows), Some(cells), Some(max_mean_milli)) = (
        mark.columns,
        mark.rows,
        mark.cells.as_deref(),
        mark.max_mean_milli,
    ) else {
        return Err(shape_error(format!(
            "color_digest '{}' is missing its grid or cells",
            mark.id
        )));
    };
    let grid = ColorDigestGrid::new(columns, rows)
        .map_err(|error| shape_error(format!("color_digest '{}': {error}", mark.id)))?;
    let expected = ColorDigest::from_hex(grid, cells)
        .map_err(|error| shape_error(format!("color_digest '{}': {error}", mark.id)))?;
    let exclude = mark.exclude_cells.clone().unwrap_or_default();
    let thresholds = ColorDigestThresholds {
        max_mean_milli,
        max_cell: mark.max_cell,
    };
    let mut test = self_test(SelfTestStatus::Passed, count);
    let (mut worst_mean, mut worst_cell) = (0_u32, 0_u32);
    let mut failure = None;
    for eval in frames {
        let observed = ColorDigest::compute(&eval.frame.scene, recognition_rect(region), grid)
            .map_err(|error| shape_error(format!("color_digest '{}': {error}", mark.id)))?;
        let distance = color_digest::distance(&expected, &observed, &exclude)
            .map_err(|error| shape_error(format!("color_digest '{}': {error}", mark.id)))?;
        worst_mean = worst_mean.max(distance.mean_milli);
        worst_cell = worst_cell.max(distance.max_cell);
        if !distance.passes(thresholds) {
            test.status = SelfTestStatus::Failed;
            test.reason = Some("self_test_failed".to_string());
            failure.get_or_insert(json!({
                "frame_id": eval.id,
                "mean_milli": distance.mean_milli,
                "max_cell": distance.max_cell,
                "max_mean_milli": max_mean_milli,
                "max_cell_threshold": mark.max_cell
            }));
        }
    }
    test.mean_milli = Some(worst_mean);
    test.max_cell = Some(worst_cell);
    test.margin = Some(SelfTestMargin::Digest {
        mean_milli: i64::from(max_mean_milli) - i64::from(worst_mean),
        max_cell: mark
            .max_cell
            .map(|limit| i64::from(limit) - i64::from(worst_cell)),
    });
    Ok((test, failure))
}

/// Workflow #396 LAB-3: an OCR mark is the OCR target the package declares for it, evaluated
/// on every frame by the recognition pack with the Lab's OCR provider, as the Runtime
/// evaluates it. A frame whose text or confidence does not pass fails the mark; a provider or
/// evaluation error fails it as `self_test_error`.
fn evaluate_ocr(
    mark: &RecordedMark,
    frames: &[EvalFrame<'_>],
    count: u32,
    provider: Arc<dyn VisionProvider>,
) -> LabResult<(SelfTest, Option<Value>)> {
    let region = region_of(mark)?;
    let spec = mark.ocr.as_ref().ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("ocr '{}' has no OCR fields", mark.id),
        )
    })?;
    let mut test = self_test(SelfTestStatus::Passed, count);
    let Some(primary) = frames.first() else {
        return Ok((test, None));
    };
    let size = primary.frame.size();
    let pack = json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": size.width, "height": size.height},
        "targets": [{
            "type": "ocr",
            "id": mark.id,
            "region": {"x": region.x, "y": region.y, "width": region.width, "height": region.height},
            "languages": spec.languages,
            "timeout_ms": spec.timeout_ms,
            "match_mode": spec.match_mode,
            "expected": spec.expected,
            "case_sensitive": spec.case_sensitive,
            "minimum_confidence": spec.minimum_confidence,
            "model_ref": spec.model_ref,
            "model_sha256": spec.model_sha256
        }]
    });
    let evaluator = load_pack_from_json_str(&pack.to_string())
        .and_then(|pack| {
            RecognitionEvaluator::with_vision_provider(
                pack,
                Arc::new(FsAssetResolver::new(PathBuf::new())),
                provider,
            )
        })
        .map_err(|error| {
            invalid(
                "validation_failed",
                format!("ocr '{}' cannot be evaluated: {error}", mark.id),
            )
        })?;
    let mut failure = None;
    for eval in frames {
        match evaluator.evaluate_target(&eval.frame.scene, &mark.id) {
            Ok(evaluation) if evaluation.passed => {}
            Ok(evaluation) => {
                test.status = SelfTestStatus::Failed;
                test.reason = Some("self_test_failed".to_string());
                let read = evaluation.ocr.as_deref();
                failure.get_or_insert(json!({
                    "frame_id": eval.id,
                    "text": read.map(|read| read.text.clone()),
                    "confidence": read.and_then(|read| read.confidence),
                    "expected": spec.expected,
                    "minimum_confidence": spec.minimum_confidence
                }));
            }
            Err(error) => {
                test.status = SelfTestStatus::Failed;
                test.reason = Some("self_test_error".to_string());
                failure.get_or_insert(json!({
                    "frame_id": eval.id,
                    "error": error.to_string()
                }));
            }
        }
    }
    Ok((test, failure))
}

/// A check is derived from its members: all_of fails on any failed member and passes when
/// all pass; any_of passes on any passed member and fails when all fail; otherwise it is
/// not evaluated.
pub(crate) fn derive_check(
    mark: &RecordedMark,
    members: &BTreeMap<String, SelfTestStatus>,
    frames: u32,
) -> (SelfTest, Option<Value>) {
    let statuses = mark
        .check_members()
        .iter()
        .map(|id| {
            (
                id.clone(),
                members
                    .get(id)
                    .copied()
                    .unwrap_or(SelfTestStatus::NotEvaluated),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let values = statuses.values().copied().collect::<Vec<_>>();
    let status = if mark.all_of.is_some() {
        if values.contains(&SelfTestStatus::Failed) {
            SelfTestStatus::Failed
        } else if values
            .iter()
            .all(|status| *status == SelfTestStatus::Passed)
        {
            SelfTestStatus::Passed
        } else {
            SelfTestStatus::NotEvaluated
        }
    } else if values.contains(&SelfTestStatus::Passed) {
        SelfTestStatus::Passed
    } else if values
        .iter()
        .all(|status| *status == SelfTestStatus::Failed)
    {
        SelfTestStatus::Failed
    } else {
        SelfTestStatus::NotEvaluated
    };
    let mut test = self_test(status, frames);
    let failure = (status == SelfTestStatus::Failed).then(|| json!({"members": statuses}));
    if status == SelfTestStatus::Failed {
        test.reason = Some("self_test_failed".to_string());
    }
    test.members = Some(statuses);
    (test, failure)
}
