// SPDX-License-Identifier: AGPL-3.0-only

//! Text located by OCR (Workflow #396 REC-1, `contracts/ocr-fields.md`, section Located text).
//!
//! An OCR target that declares `locate` compares its `expected` names with every block the
//! provider read, one block at a time, after the identity normalization of candidate names.
//! The best hit's block, or the declared cell that holds the block's center, becomes the click
//! box of a `target` or `offset` click. Reads of one consumer (one page recognition, one guard
//! evaluation) are remembered per request within one [`SceneEvaluation`], so one frame and one
//! request call the provider once, and stay within the consumer's budget of
//! [`TEXT_READ_MAX_CALLS`] calls and one frame of summed area.

use crate::codes::{RecognitionPackCode, RecognitionPackLocation, TextReadBudget};
use crate::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_contract::outcome::Outcome;
use std::collections::BTreeMap;

/// The most provider calls one consumer's text reads make (contract constant).
pub const TEXT_READ_MAX_CALLS: u32 = 4;
/// The most cells one `locate.cells` grid declares.
const TEXT_LOCATE_MAX_CELLS: i64 = 64;
/// The largest `line_merge.gap_factor` (the model folder's own range).
const TEXT_LOCATE_MAX_GAP_FACTOR: u32 = 16;

/// Schema 0.7 `locate` of an OCR target. Every member is written by the pack; there is no
/// Runtime default.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocate {
    pub normalize: OcrLocateNormalize,
    pub blocks: OcrLocateBlocks,
    /// Required with `blocks: "detect"`, refused with `blocks: "region"`.
    #[serde(default)]
    pub line_merge: Option<OcrLocateLineMerge>,
    #[serde(default)]
    pub cells: Option<OcrLocateCells>,
}

/// The normalization applied to blocks and `expected` names alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum OcrLocateNormalize {
    /// The identity normalization of candidate names ([`normalize_name`] without confusions).
    #[serde(rename = "identity_v1")]
    IdentityV1,
}

/// How the provider cuts the region into blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrLocateBlocks {
    /// The detector runs in the region and every box is read with its own rectangle.
    Detect,
    /// The whole region is one block whose rectangle is the region.
    Region,
}

/// `"none"` or `{"gap_factor": n}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OcrLocateLineMerge {
    Keyword(OcrLocateLineMergeKeyword),
    Factor(OcrLocateGapFactor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum OcrLocateLineMergeKeyword {
    /// No two boxes merge.
    #[serde(rename = "none")]
    Never,
}

/// The folder's line merge with this factor (0..=16) in place of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocateGapFactor {
    pub gap_factor: u32,
}

/// A grid of cells relative to the region's top-left corner; cell `k = row * columns + column`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocateCells {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub pitch_x: i64,
    pub pitch_y: i64,
    pub columns: i64,
    pub rows: i64,
}

impl OcrLocate {
    /// The provider request's block mode.
    pub fn provider_blocks(&self) -> OcrProviderBlocks {
        match (self.blocks, self.line_merge) {
            (OcrLocateBlocks::Region, _) => OcrProviderBlocks::Region,
            (OcrLocateBlocks::Detect, Some(OcrLocateLineMerge::Factor(factor))) => {
                OcrProviderBlocks::Detect {
                    line_merge_gap_factor: Some(factor.gap_factor),
                }
            }
            (OcrLocateBlocks::Detect, Some(OcrLocateLineMerge::Keyword(_)) | None) => {
                OcrProviderBlocks::Detect {
                    line_merge_gap_factor: None,
                }
            }
        }
    }
}

/// How a provider cuts an OCR request's region into blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcrProviderBlocks {
    /// An OCR target without `locate`: a full-frame region is detected and merged by the model
    /// folder, any smaller region is one block, exactly as before `locate` existed.
    #[default]
    Implicit,
    /// The whole region is one block.
    Region,
    /// The detector runs in the region; `None` merges no boxes, `Some(n)` merges with factor
    /// `n` in place of the folder's.
    Detect { line_merge_gap_factor: Option<u32> },
}

/// A `Detect` read's box count before the model folder's `max_boxes` kept the first `limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrProviderDetection {
    pub detected: u32,
    pub limit: u32,
}

impl OcrProviderDetection {
    /// Boxes beyond `max_boxes` were dropped unread.
    pub fn truncated(self) -> bool {
        self.detected > self.limit
    }
}

/// Where a locate OCR target found its text; present only when the target passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OcrLocatedEvidence {
    /// The hit block's index among the evaluation's sorted `blocks`.
    pub block_index: usize,
    pub rect: PackRect,
    pub confidence_milli: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_rect: Option<PackRect>,
    /// The detector found more boxes than the folder's `max_boxes`.
    pub truncated: bool,
}

/// One consumer's remembered text reads within one `SceneEvaluation`.
#[derive(Default)]
pub(crate) struct TextReads {
    results: HashMap<TextReadKey, RecognitionPackResult<OcrProviderResult>>,
    calls: u32,
    area: u64,
}

/// Requests are the same when region, model, languages, timeout and block mode are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextReadKey {
    region: PackRect,
    model_ref: String,
    model_sha256: String,
    languages: Vec<String>,
    timeout_ms: u64,
    blocks: OcrProviderBlocks,
}

impl SceneEvaluation<'_> {
    /// The provider's result for this request on this frame: remembered after the first call,
    /// so a later target with the same request reads it without a call. Before a call the
    /// consumer's budget is checked; an exceeded budget calls nothing. A remembered result
    /// carries its PP-OCR reports only once, with the call that produced them.
    pub(crate) fn read_text_once(
        &self,
        provider: &dyn VisionProvider,
        target: &OcrTarget,
        region: PackRect,
        blocks: OcrProviderBlocks,
    ) -> RecognitionPackResult<OcrProviderResult> {
        let key = TextReadKey {
            region,
            model_ref: target.model_ref.clone(),
            model_sha256: target.model_sha256.clone(),
            languages: target.languages.clone(),
            timeout_ms: target.timeout_ms,
            blocks,
        };
        if let Some(remembered) = self.text_reads.borrow().results.get(&key) {
            return match remembered {
                Ok(result) => {
                    let mut result = result.clone();
                    result.ppocr_diagnostics = Vec::new();
                    Ok(result)
                }
                Err(error) => {
                    let mut error = error.clone();
                    error.ppocr_diagnostics = Vec::new();
                    Err(error)
                }
            };
        }
        let frame_area = u64::from(self.scene.width()) * u64::from(self.scene.height());
        let area = rect_area(region);
        {
            let reads = self.text_reads.borrow();
            if reads.calls >= TEXT_READ_MAX_CALLS {
                return Err(text_read_budget_exceeded(
                    &target.id,
                    TextReadBudget::Calls,
                    u64::from(reads.calls),
                    u64::from(TEXT_READ_MAX_CALLS),
                ));
            }
            if reads.area.saturating_add(area) > frame_area {
                return Err(text_read_budget_exceeded(
                    &target.id,
                    TextReadBudget::Area,
                    reads.area,
                    frame_area,
                ));
            }
        }
        let result = provider
            .read_text(OcrProviderRequest {
                frame: provider_frame(self.scene),
                region,
                languages: &target.languages,
                timeout_ms: target.timeout_ms,
                model_ref: &target.model_ref,
                model_sha256: &target.model_sha256,
                blocks,
            })
            .map_err(|err| provider_error(&target.id, "ocr", err));
        let mut reads = self.text_reads.borrow_mut();
        reads.calls += 1;
        reads.area = reads.area.saturating_add(area);
        reads.results.insert(key, result.clone());
        result
    }
}

impl RecognitionEvaluator {
    /// Evaluates a locate OCR target: one block-by-block comparison of the remembered read.
    pub(crate) fn evaluate_located_text(
        &self,
        context: &SceneEvaluation<'_>,
        target: &OcrTarget,
        locate: &OcrLocate,
    ) -> RecognitionPackResult<TargetEvaluation> {
        let (region, region_evidence) = context.ocr_region(target)?;
        (|| {
            let provider = self.vision_provider.as_ref().ok_or_else(|| {
                RecognitionPackError::fatal_with_code(
                    RecognitionPackErrorCode::VisionProviderMissing,
                    format!("ocr target '{}' has no injected vision provider", target.id),
                )
            })?;
            provider
                .require_ocr_model(&target.model_ref, &target.model_sha256)
                .map_err(|error| provider_error(&target.id, "ocr admission", error))?;
            let result = context.read_text_once(
                provider.as_ref(),
                target,
                region,
                locate.provider_blocks(),
            )?;
            let detection = result.detection;
            let ocr = validate_ocr_result(result, region)?;
            let truncated = detection.is_some_and(OcrProviderDetection::truncated);
            let mut matched_expected = None;
            let mut located = None;
            let failure = match find_located_text(&ocr.blocks, target) {
                None if truncated => {
                    let detection = detection.unwrap_or(OcrProviderDetection {
                        detected: 0,
                        limit: 0,
                    });
                    return Err(RecognitionPackError::text_read(
                        Outcome::new(RecognitionPackCode::TextReadBlocksTruncated)
                            .target_id(target.id.as_str())
                            .count(detection.detected)
                            .limit_count(detection.limit)
                            .stage(RecognitionPackLocation::TextLocateEvaluate),
                    ));
                }
                None => Some(
                    Outcome::new(RecognitionPackCode::TextLocateNotFound)
                        .target_id(target.id.as_str())
                        .count(ocr.blocks.len())
                        .stage(RecognitionPackLocation::TextLocateEvaluate),
                ),
                Some(hit) => {
                    matched_expected = Some(hit.expected.clone());
                    let rect = ocr.blocks[hit.block_index].rect;
                    let cell = match &locate.cells {
                        Some(cells) => Some(located_cell(cells, region, rect)?),
                        None => None,
                    };
                    match cell {
                        Some(None) => Some(
                            Outcome::new(RecognitionPackCode::TextLocateOutsideCells)
                                .target_id(target.id.as_str())
                                .stage(RecognitionPackLocation::TextLocateEvaluate),
                        ),
                        Some(Some((cell_index, cell_rect))) => {
                            located = Some(OcrLocatedEvidence {
                                block_index: hit.block_index,
                                rect,
                                confidence_milli: hit.confidence_milli,
                                cell_index: Some(cell_index),
                                cell_rect: Some(cell_rect),
                                truncated,
                            });
                            None
                        }
                        None => {
                            located = Some(OcrLocatedEvidence {
                                block_index: hit.block_index,
                                rect,
                                confidence_milli: hit.confidence_milli,
                                cell_index: None,
                                cell_rect: None,
                                truncated,
                            });
                            None
                        }
                    }
                }
            };
            let passed = located.is_some();
            let message = match (&failure, passed) {
                (_, true) => "ocr text located",
                (Some(failure), false)
                    if failure.is(RecognitionPackCode::TextLocateOutsideCells) =>
                {
                    "ocr text located outside every cell"
                }
                _ => "ocr text not located",
            }
            .to_string();
            Ok(TargetEvaluation {
                id: target.id.clone(),
                kind: TargetKind::Ocr,
                sample_evaluations: Vec::new(),
                sampling: None,
                passed,
                template: None,
                color: None,
                ocr: Some(Box::new(OcrEvaluation {
                    ppocr_diagnostics: ocr.ppocr_diagnostics,
                    raw_text: ocr.raw_text,
                    block_source_order: ocr.block_source_order,
                    region: Box::new(region_evidence.clone()),
                    text: ocr.text,
                    confidence: ocr.confidence,
                    matched_expected,
                    match_mode: target.match_mode,
                    blocks: ocr.blocks,
                    located,
                })),
                nn: None,
                color_digest: None,
                composite: None,
                failure,
                message,
            })
        })()
        .map_err(|error: RecognitionPackError| error.with_region(&region_evidence))
    }

    /// Adds what `target_id` reads to one consumer's static text-read count: a locate OCR
    /// target, or the locate members of a check. Each distinct request counts once; a
    /// template-relative region is its own request. Another target reads nothing here.
    pub fn count_text_reads(
        &self,
        count: &mut TextReadCount,
        target_id: &str,
    ) -> Result<(), TextReadBudgetOverrun> {
        let frame = self
            .pack
            .coordinate_space
            .map(|space| (space.width, space.height));
        let mut add = |target: &OcrTarget| -> Result<(), TextReadBudgetOverrun> {
            let Some(locate) = &target.locate else {
                return Ok(());
            };
            let (region, area) = match (&target.region, frame) {
                (PackRegion::Rect(rect), _) => (StaticTextRegion::Rect(*rect), rect_area(*rect)),
                (PackRegion::Keyword(value), Some((width, height))) if value == "full_frame" => (
                    StaticTextRegion::Rect(PackRect {
                        x: 0,
                        y: 0,
                        width: i32::try_from(width).unwrap_or(i32::MAX),
                        height: i32::try_from(height).unwrap_or(i32::MAX),
                    }),
                    u64::from(width) * u64::from(height),
                ),
                (
                    PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                        width,
                        height,
                        ..
                    }),
                    _,
                ) => (
                    StaticTextRegion::Relative(target.id.clone()),
                    rect_area(PackRect {
                        x: 0,
                        y: 0,
                        width: *width,
                        height: *height,
                    }),
                ),
                // An unusable region is refused by the pack's own validation.
                (PackRegion::Keyword(_), _) => return Ok(()),
            };
            let read = StaticTextRead {
                region,
                model_ref: target.model_ref.clone(),
                model_sha256: target.model_sha256.clone(),
                languages: target.languages.clone(),
                timeout_ms: target.timeout_ms,
                blocks: locate.provider_blocks(),
            };
            if count.reads.contains(&read) {
                return Ok(());
            }
            let calls = u64::try_from(count.reads.len()).unwrap_or(u64::MAX);
            if calls >= u64::from(TEXT_READ_MAX_CALLS) {
                return Err(TextReadBudgetOverrun {
                    budget: TextReadBudget::Calls,
                    used: calls,
                    limit: u64::from(TEXT_READ_MAX_CALLS),
                });
            }
            let frame_area =
                frame.map_or(0, |(width, height)| u64::from(width) * u64::from(height));
            if count.area.saturating_add(area) > frame_area {
                return Err(TextReadBudgetOverrun {
                    budget: TextReadBudget::Area,
                    used: count.area,
                    limit: frame_area,
                });
            }
            count.area = count.area.saturating_add(area);
            count.reads.insert(read);
            Ok(())
        };
        match self.target(target_id) {
            Ok(RecognitionTarget::Ocr(target)) => add(target),
            Ok(RecognitionTarget::Composite(composite)) => {
                for member in &composite.members {
                    if let Ok(RecognitionTarget::Ocr(target)) = self.target(member) {
                        add(target)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Whether `target_id` is a locate OCR target.
    pub fn locates_text(&self, target_id: &str) -> bool {
        matches!(
            self.target(target_id),
            Ok(RecognitionTarget::Ocr(OcrTarget {
                locate: Some(_),
                ..
            }))
        )
    }
}

/// One consumer's distinct static text reads and their summed area.
#[derive(Debug, Clone, Default)]
pub struct TextReadCount {
    reads: HashSet<StaticTextRead>,
    area: u64,
}

/// A consumer whose text reads exceed a budget: `used` before the read that exceeds `limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextReadBudgetOverrun {
    pub budget: TextReadBudget,
    pub used: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StaticTextRegion {
    Rect(PackRect),
    /// A template-relative region, by its target: its position is only known per frame.
    Relative(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StaticTextRead {
    region: StaticTextRegion,
    model_ref: String,
    model_sha256: String,
    languages: Vec<String>,
    timeout_ms: u64,
    blocks: OcrProviderBlocks,
}

fn text_read_budget_exceeded(
    target_id: &str,
    budget: TextReadBudget,
    used: u64,
    limit: u64,
) -> RecognitionPackError {
    RecognitionPackError::text_read(
        Outcome::new(RecognitionPackCode::TextReadBudgetExceeded)
            .target_id(target_id)
            .text_read_budget(budget)
            .count(used)
            .limit_count(limit)
            .stage(RecognitionPackLocation::TextReadRequest),
    )
}

fn rect_area(rect: PackRect) -> u64 {
    u64::try_from(rect.width).unwrap_or(0) * u64::try_from(rect.height).unwrap_or(0)
}

/// The best hit among the blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocatedHit {
    block_index: usize,
    confidence_milli: u32,
    /// The `expected` value, as declared, that the block matched.
    expected: String,
}

/// Compares every block on its own, never across blocks. A block matches when its
/// normalized text equals (`exact`) or contains (`contains`) a normalized `expected` value; a
/// block without a normalized text (longer than 128 characters, blank or holding a control
/// character) matches nothing. A match is a hit only with a confidence of at least
/// `minimum_confidence`. The highest confidence in milli wins; on a tie the earlier block.
fn find_located_text(blocks: &[OcrTextEvidence], target: &OcrTarget) -> Option<LocatedHit> {
    let confusions = BTreeMap::new();
    let expected = target
        .expected
        .iter()
        .filter_map(|value| normalize_name(value, &confusions).map(|name| (value, name)))
        .collect::<Vec<_>>();
    let mut best: Option<LocatedHit> = None;
    for (block_index, block) in blocks.iter().enumerate() {
        let Some(text) = normalize_name(&block.text, &confusions) else {
            continue;
        };
        let Some((declared, _)) = expected.iter().find(|(_, name)| match target.match_mode {
            OcrMatchMode::Exact => text == *name,
            OcrMatchMode::Contains => text.contains(name.as_str()),
        }) else {
            continue;
        };
        let Some(confidence) = block
            .confidence
            .filter(|confidence| *confidence >= target.minimum_confidence)
        else {
            continue;
        };
        let confidence_milli =
            u32::from(actingcommand_contract::ocr_confidence_milli(Some(confidence)).unwrap_or(0));
        if best
            .as_ref()
            .is_none_or(|best| confidence_milli > best.confidence_milli)
        {
            best = Some(LocatedHit {
                block_index,
                confidence_milli,
                expected: (*declared).clone(),
            });
        }
    }
    best
}

/// The cell that holds the block's center `(x + floor(w/2), y + floor(h/2))`, as its index and
/// frame rectangle; `None` when no cell holds it.
fn located_cell(
    cells: &OcrLocateCells,
    region: PackRect,
    block: PackRect,
) -> RecognitionPackResult<Option<(u32, PackRect)>> {
    let center_x = i64::from(block.x) + i64::from(block.width) / 2;
    let center_y = i64::from(block.y) + i64::from(block.height) / 2;
    for row in 0..cells.rows {
        for column in 0..cells.columns {
            let x = i64::from(region.x) + cells.x + column * cells.pitch_x;
            let y = i64::from(region.y) + cells.y + row * cells.pitch_y;
            if (x..x + cells.width).contains(&center_x) && (y..y + cells.height).contains(&center_y)
            {
                let rect = match (
                    i32::try_from(x),
                    i32::try_from(y),
                    i32::try_from(cells.width),
                    i32::try_from(cells.height),
                ) {
                    (Ok(x), Ok(y), Ok(width), Ok(height)) => PackRect {
                        x,
                        y,
                        width,
                        height,
                    },
                    _ => {
                        return Err(RecognitionPackError::fatal(
                            "locate cell exceeds the i32 range",
                        ));
                    }
                };
                let index = u32::try_from(row * cells.columns + column)
                    .map_err(|_| RecognitionPackError::fatal("locate cell index overflow"))?;
                return Ok(Some((index, rect)));
            }
        }
    }
    Ok(None)
}

/// A refused `locate` declaration: its JSON pointer in the recognition pack, the reason, and a
/// developer explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocateRefusal {
    pub(crate) pointer: String,
    pub(crate) reason: ResourceDeclarationReason,
    pub(crate) explanation: String,
}

fn refuse(
    pointer: String,
    reason: ResourceDeclarationReason,
    explanation: impl Into<String>,
) -> Result<(), LocateRefusal> {
    Err(LocateRefusal {
        pointer,
        reason,
        explanation: explanation.into(),
    })
}

fn pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Whether `target_id` reads located text: a locate OCR target, or a check with one as member.
fn reads_located_text(pack: &RecognitionPack, target_id: &str) -> bool {
    let locate = |id: &str| {
        pack.targets.iter().any(|target| {
            matches!(
                target,
                RecognitionTarget::Ocr(OcrTarget {
                    id: candidate,
                    locate: Some(_),
                    ..
                }) if candidate == id
            )
        })
    };
    locate(target_id)
        || pack.targets.iter().any(|target| {
            matches!(
                target,
                RecognitionTarget::Composite(composite)
                    if composite.id == target_id
                        && composite.members.iter().any(|member| locate(member))
            )
        })
}

/// The declaration rules of `locate` (`contracts/ocr-fields.md`, section Located text). The
/// first refused declaration is returned with its pointer.
pub(crate) fn validate_locate_declarations(pack: &RecognitionPack) -> Result<(), LocateRefusal> {
    let confusions = BTreeMap::new();
    for (index, target) in pack.targets.iter().enumerate() {
        let RecognitionTarget::Ocr(target) = target else {
            continue;
        };
        let Some(locate) = &target.locate else {
            continue;
        };
        let base = format!("/targets/{index}");
        if pack.schema_version != SCHEMA_0_7 {
            refuse(
                format!("{base}/locate"),
                ResourceDeclarationReason::UnconsumedField,
                format!("locate requires schema_version '{SCHEMA_0_7}'"),
            )?;
        }
        if target.case_sensitive {
            refuse(
                format!("{base}/case_sensitive"),
                ResourceDeclarationReason::InvalidValue,
                "a locate target compares normalized lowercase text; case_sensitive must be false",
            )?;
        }
        for (expected_index, expected) in target.expected.iter().enumerate() {
            if normalize_name(expected, &confusions).is_none() {
                refuse(
                    format!("{base}/expected/{expected_index}"),
                    ResourceDeclarationReason::InvalidValue,
                    "an expected value normalizes to nothing: blank, longer than 128 characters or holding a control character",
                )?;
            }
        }
        let full_frame =
            matches!(&target.region, PackRegion::Keyword(value) if value == "full_frame");
        match (locate.blocks, locate.line_merge) {
            (OcrLocateBlocks::Region, _) if full_frame => refuse(
                format!("{base}/locate/blocks"),
                ResourceDeclarationReason::InvalidValue,
                "blocks 'region' reads a full_frame region with the detector anyway",
            )?,
            (OcrLocateBlocks::Region, Some(_)) => refuse(
                format!("{base}/locate/line_merge"),
                ResourceDeclarationReason::UnconsumedField,
                "line_merge is declared only with blocks 'detect'",
            )?,
            (OcrLocateBlocks::Detect, None) => refuse(
                format!("{base}/locate/line_merge"),
                ResourceDeclarationReason::MissingField,
                "blocks 'detect' requires line_merge",
            )?,
            (OcrLocateBlocks::Detect, Some(OcrLocateLineMerge::Factor(factor)))
                if factor.gap_factor > TEXT_LOCATE_MAX_GAP_FACTOR =>
            {
                refuse(
                    format!("{base}/locate/line_merge/gap_factor"),
                    ResourceDeclarationReason::InvalidValue,
                    format!("gap_factor must be in 0..={TEXT_LOCATE_MAX_GAP_FACTOR}"),
                )?
            }
            _ => {}
        }
        if let Some(cells) = &locate.cells {
            let region = match &target.region {
                PackRegion::Rect(rect) => Some((i64::from(rect.width), i64::from(rect.height))),
                PackRegion::Keyword(_) => pack
                    .coordinate_space
                    .map(|space| (i64::from(space.width), i64::from(space.height))),
                PackRegion::TemplateRelative(TemplateRelativeRegion::TemplateRelative {
                    width,
                    height,
                    ..
                }) => Some((i64::from(*width), i64::from(*height))),
            };
            validate_cells(cells, region, &format!("{base}/locate/cells"))?;
        }
    }
    for id in pack.target_consensus.keys() {
        if reads_located_text(pack, id) {
            refuse(
                format!("/target_consensus/{}", pointer_token(id)),
                ResourceDeclarationReason::InvalidValue,
                "a locate target cannot declare target consensus",
            )?;
        }
    }
    for (layout_index, layout) in pack.candidate_layouts.iter().enumerate() {
        for (slot_index, slot) in layout.slots.iter().enumerate() {
            for (name, target_id) in &slot.targets {
                if reads_located_text(pack, target_id) {
                    refuse(
                        format!(
                            "/candidate_layouts/{layout_index}/slots/{slot_index}/targets/{}",
                            pointer_token(name)
                        ),
                        ResourceDeclarationReason::InvalidValue,
                        "a locate target cannot be a candidate layout slot target or feature",
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Each cell lies inside the region, cells never overlap, and the grid has 1 to 64 cells.
fn validate_cells(
    cells: &OcrLocateCells,
    region: Option<(i64, i64)>,
    pointer: &str,
) -> Result<(), LocateRefusal> {
    let invalid = |field: &str, explanation: &str| {
        refuse(
            if field.is_empty() {
                pointer.to_owned()
            } else {
                format!("{pointer}/{field}")
            },
            ResourceDeclarationReason::InvalidValue,
            explanation,
        )
    };
    if cells.x < 0 {
        invalid("x", "cells.x must be at least 0")?;
    }
    if cells.y < 0 {
        invalid("y", "cells.y must be at least 0")?;
    }
    if cells.width < 1 {
        invalid("width", "cells.width must be at least 1")?;
    }
    if cells.height < 1 {
        invalid("height", "cells.height must be at least 1")?;
    }
    if cells.pitch_x < cells.width {
        invalid("pitch_x", "cells overlap: pitch_x is below width")?;
    }
    if cells.pitch_y < cells.height {
        invalid("pitch_y", "cells overlap: pitch_y is below height")?;
    }
    if cells.columns < 1 {
        invalid("columns", "cells.columns must be at least 1")?;
    }
    if cells.rows < 1 {
        invalid("rows", "cells.rows must be at least 1")?;
    }
    if cells
        .columns
        .checked_mul(cells.rows)
        .is_none_or(|count| count > TEXT_LOCATE_MAX_CELLS)
    {
        invalid("", "a cell grid has at most 64 cells")?;
    }
    if let Some((region_width, region_height)) = region {
        let extent = |origin: i64, pitch: i64, count: i64, size: i64| {
            i128::from(origin) + i128::from(pitch) * i128::from(count - 1) + i128::from(size)
        };
        if extent(cells.x, cells.pitch_x, cells.columns, cells.width) > i128::from(region_width)
            || extent(cells.y, cells.pitch_y, cells.rows, cells.height) > i128::from(region_height)
        {
            invalid("", "the cell grid extends beyond the region")?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "text_locate_tests.rs"]
mod tests;
