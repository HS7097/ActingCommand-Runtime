// SPDX-License-Identifier: AGPL-3.0-only

//! The workload file (design §1.3): the frames to read and, per frame, the requests to send to
//! the engine. A generator script outside this repository writes it from the offline truth set;
//! the bench reads only this file and the PNG frames it names. Every object refuses members it
//! does not define.
//!
//! Workflow #407 GPU-0b: a workload may also carry `template_search`, the templates the template
//! mode searches over every whole frame (design §1.2 "模板匹配"). Each template's PNG travels
//! inside the workload as hexadecimal text with its SHA-256, so the bench still reads nothing but
//! this file and the frames.

use crate::error::{BenchCode, BenchError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The schema spelling the bench reads.
pub const WORKLOAD_SCHEMA: &str = "actingcommand.vision-ep-bench.workload/1";
/// The engine's request timeout ceiling (`vision-ffi`'s `MAX_REQUEST_TIMEOUT_MS`).
pub const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;
/// The largest template scale factor the workload accepts, in milli.
pub const MAX_TEMPLATE_SCALE_MILLI: u32 = 4_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub schema: String,
    pub model: WorkloadModel,
    pub frames: Vec<WorkloadFrame>,
    /// Free text about where the workload came from (generator, sources, commits).
    #[serde(default)]
    pub provenance: BTreeMap<String, String>,
    /// Workflow #407 GPU-0b: what the template mode searches; the OCR mode ignores it.
    #[serde(default)]
    pub template_search: Option<TemplateSearchSpec>,
}

/// Workflow #407 GPU-0b: the templates of the template mode and the scale sets each one runs
/// with (every template runs without and with its mask, once per scale set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSearchSpec {
    /// E.g. `[[1000], [900, 1000, 1100]]`: one scale, and three scales.
    pub scale_sets_milli: Vec<Vec<u32>>,
    /// The overlap suppression of today's direct enumeration (`match_template_all`).
    pub suppress_iou_milli: u16,
    pub templates: Vec<WorkloadTemplate>,
}

/// One template PNG of a resource repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadTemplate {
    pub id: String,
    /// The size class of design §1.2 the template stands for (about 24, 48, 96 or 160 px).
    pub size_class_px: u32,
    /// Where the PNG came from (repository, commit, path), for the record.
    pub source: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub metric: TemplateMetric,
    /// The threshold of the template's own declaration; a position whose exact score reaches it
    /// is a match.
    pub threshold: f32,
    /// The gray range the masked runs score.
    pub mask: WorkloadMask,
    /// The PNG's bytes as lowercase hexadecimal text.
    pub png_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMetric {
    CcorrNormed,
    CcoeffNormed,
}

/// A pack's `{"type": "range", "lower", "upper"}` mask: template pixels of a gray value in
/// `lower..=upper` are scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadMask {
    pub lower: u8,
    pub upper: u8,
}

/// The one OCR model the workload's requests name, by folder and content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadModel {
    pub model_ref: String,
    pub model_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadFrame {
    pub id: String,
    /// An absolute path to a PNG frame.
    pub path: PathBuf,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    /// A frame no request of its pack is expected to read (a negative of the truth set).
    #[serde(default)]
    pub negative: bool,
    pub requests: Vec<WorkloadRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadRequest {
    pub id: String,
    pub region: WorkloadRect,
    pub blocks: WorkloadBlocks,
    pub languages: Vec<String>,
    /// The timeout the bench gives the engine (1..=60000 ms).
    pub timeout_ms: u64,
    /// The truth text of a field, compared exactly.
    #[serde(default)]
    pub expected_text: Option<String>,
    /// The timeout the pack declares for the field, kept for the record only.
    #[serde(default)]
    pub declared_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// How the engine cuts the region into blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadBlocks {
    /// The production default: the detector on a full-frame region, one block otherwise.
    Implicit,
    /// One block, the region.
    Region,
    /// The detector inside the region with the model folder's own line merge.
    Detect,
}

/// Whether a request reads the whole frame through the detector or one region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    FullFrame,
    Roi,
}

impl WorkloadRequest {
    pub fn kind(&self, frame: &WorkloadFrame) -> RequestKind {
        let full = self.region
            == WorkloadRect {
                x: 0,
                y: 0,
                width: i32::try_from(frame.width).unwrap_or(i32::MAX),
                height: i32::try_from(frame.height).unwrap_or(i32::MAX),
            };
        match self.blocks {
            WorkloadBlocks::Detect => RequestKind::FullFrame,
            WorkloadBlocks::Implicit if full => RequestKind::FullFrame,
            WorkloadBlocks::Implicit | WorkloadBlocks::Region => RequestKind::Roi,
        }
    }
}

/// Parses and validates a workload.
pub fn parse_workload(text: &str) -> Result<Workload, BenchError> {
    let workload: Workload = serde_json::from_str(text)
        .map_err(|error| invalid(format!("the workload does not parse: {error}")))?;
    workload.validate()?;
    Ok(workload)
}

impl Workload {
    pub fn validate(&self) -> Result<(), BenchError> {
        if self.schema != WORKLOAD_SCHEMA {
            return Err(invalid(format!(
                "schema is {:?}, the bench reads {WORKLOAD_SCHEMA:?}",
                self.schema
            )));
        }
        if self.model.model_ref.trim().is_empty() {
            return Err(invalid("model.model_ref is empty"));
        }
        if !is_sha256(&self.model.model_sha256) {
            return Err(invalid(
                "model.model_sha256 is not 64 lowercase hexadecimal characters",
            ));
        }
        if self.frames.is_empty() {
            return Err(invalid("the workload has no frame"));
        }
        let mut frame_ids = BTreeSet::new();
        for frame in &self.frames {
            if frame.id.trim().is_empty() || !frame_ids.insert(frame.id.as_str()) {
                return Err(invalid(format!(
                    "frame id {:?} is empty or repeated",
                    frame.id
                )));
            }
            if !frame.path.is_absolute() {
                return Err(invalid(format!(
                    "frame {}: path {} is not absolute",
                    frame.id,
                    frame.path.display()
                )));
            }
            if !is_sha256(&frame.sha256) {
                return Err(invalid(format!(
                    "frame {}: sha256 is not 64 lowercase hexadecimal characters",
                    frame.id
                )));
            }
            let (Ok(width), Ok(height)) = (i32::try_from(frame.width), i32::try_from(frame.height))
            else {
                return Err(invalid(format!("frame {}: size exceeds i32", frame.id)));
            };
            if width == 0 || height == 0 {
                return Err(invalid(format!("frame {}: size is zero", frame.id)));
            }
            if frame.requests.is_empty() {
                return Err(invalid(format!("frame {}: no request", frame.id)));
            }
            let mut request_ids = BTreeSet::new();
            for request in &frame.requests {
                let place = format!("frame {} request {:?}", frame.id, request.id);
                if request.id.trim().is_empty() || !request_ids.insert(request.id.as_str()) {
                    return Err(invalid(format!("{place}: id is empty or repeated")));
                }
                let rect = request.region;
                let inside = rect.x >= 0
                    && rect.y >= 0
                    && rect.width > 0
                    && rect.height > 0
                    && rect
                        .x
                        .checked_add(rect.width)
                        .is_some_and(|right| right <= width)
                    && rect
                        .y
                        .checked_add(rect.height)
                        .is_some_and(|bottom| bottom <= height);
                if !inside {
                    return Err(invalid(format!(
                        "{place}: region {rect:?} is empty or outside the {width}x{height} frame"
                    )));
                }
                if request.languages.is_empty()
                    || request
                        .languages
                        .iter()
                        .any(|language| language.trim().is_empty())
                {
                    return Err(invalid(format!(
                        "{place}: languages must hold at least one non-blank value"
                    )));
                }
                if request.timeout_ms == 0 || request.timeout_ms > MAX_REQUEST_TIMEOUT_MS {
                    return Err(invalid(format!(
                        "{place}: timeout_ms must be in 1..={MAX_REQUEST_TIMEOUT_MS}"
                    )));
                }
            }
        }
        if let Some(spec) = &self.template_search {
            spec.validate()?;
        }
        Ok(())
    }

    /// The number of requests of each kind.
    pub fn request_counts(&self) -> (usize, usize) {
        let mut full = 0;
        let mut roi = 0;
        for frame in &self.frames {
            for request in &frame.requests {
                match request.kind(frame) {
                    RequestKind::FullFrame => full += 1,
                    RequestKind::Roi => roi += 1,
                }
            }
        }
        (full, roi)
    }
}

impl TemplateSearchSpec {
    fn validate(&self) -> Result<(), BenchError> {
        if self.scale_sets_milli.is_empty() {
            return Err(invalid("template_search.scale_sets_milli is empty"));
        }
        for set in &self.scale_sets_milli {
            let distinct: BTreeSet<u32> = set.iter().copied().collect();
            if set.is_empty()
                || distinct.len() != set.len()
                || set
                    .iter()
                    .any(|factor| *factor == 0 || *factor > MAX_TEMPLATE_SCALE_MILLI)
            {
                return Err(invalid(format!(
                    "template_search scale set {set:?} must hold distinct factors in 1..={MAX_TEMPLATE_SCALE_MILLI}"
                )));
            }
        }
        if self.suppress_iou_milli > 999 {
            return Err(invalid(
                "template_search.suppress_iou_milli must be in 0..=999",
            ));
        }
        if self.templates.is_empty() {
            return Err(invalid("template_search.templates is empty"));
        }
        let mut ids = BTreeSet::new();
        for template in &self.templates {
            let place = format!("template {:?}", template.id);
            if template.id.trim().is_empty() || !ids.insert(template.id.as_str()) {
                return Err(invalid(format!("{place}: id is empty or repeated")));
            }
            if !is_sha256(&template.sha256) {
                return Err(invalid(format!(
                    "{place}: sha256 is not 64 lowercase hexadecimal characters"
                )));
            }
            if template.width == 0 || template.height == 0 {
                return Err(invalid(format!("{place}: size is zero")));
            }
            if !template.threshold.is_finite()
                || template.threshold <= 0.0
                || template.threshold > 1.0
            {
                return Err(invalid(format!("{place}: threshold must be in (0, 1]")));
            }
            if template.mask.lower > template.mask.upper {
                return Err(invalid(format!("{place}: mask lower exceeds upper")));
            }
            if template.png_hex.is_empty() {
                return Err(invalid(format!("{place}: png_hex is empty")));
            }
        }
        Ok(())
    }
}

/// The bytes of lowercase hexadecimal text (a template's `png_hex`).
pub fn decode_hex(text: &str) -> Result<Vec<u8>, BenchError> {
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    if !text.len().is_multiple_of(2) {
        return Err(invalid("hexadecimal text has an odd length"));
    }
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    pairs
        .iter()
        .map(|&[first, second]| match (digit(first), digit(second)) {
            (Some(high), Some(low)) => Ok((high << 4) | low),
            _ => Err(invalid(
                "hexadecimal text holds a character outside 0-9 and a-f",
            )),
        })
        .collect()
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(message: impl Into<String>) -> BenchError {
    BenchError::new(BenchCode::WorkloadInvalid, message)
}
