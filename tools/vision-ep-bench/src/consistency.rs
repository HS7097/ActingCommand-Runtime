// SPDX-License-Identifier: AGPL-3.0-only

//! The consistency section (design §1.2 and §1.6 item 2): this run's engine readings against the
//! `cpu-1t` baseline report, request by request: the same text, the confidence difference in
//! milli, the detected box count, the text set and the boxes' IoU; and the gate of §1.6.

use crate::error::{BenchCode, BenchError};
use crate::report::{BlockOutput, ReportStatus, RequestOutput, RequestRecord};
use crate::workload::{RequestKind, WorkloadRect};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Design §1.6 item 2: at most this many ROI texts may differ from the baseline (36/37).
pub const GATE_ROI_TEXT_DIFFERENCES: usize = 1;
/// Design §1.6 item 2: at most this many frames may differ in box count (32/34).
pub const GATE_BOX_COUNT_DIFFERENCES: usize = 2;
/// Design §1.6 item 2: the largest confidence difference, in milli.
pub const GATE_CONFIDENCE_DELTA_MILLI: u32 = 20;

/// The part of a baseline report the comparison reads.
#[derive(Debug, Clone, Deserialize)]
pub struct BaselineReport {
    pub status: ReportStatus,
    pub variant: BaselineVariant,
    pub engine: BaselineEngine,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaselineVariant {
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaselineEngine {
    pub requests: Vec<RequestRecord>,
}

pub fn parse_baseline(text: &str) -> Result<BaselineReport, BenchError> {
    let baseline: BaselineReport = serde_json::from_str(text).map_err(|error| {
        BenchError::new(
            BenchCode::BaselineInvalid,
            format!("the baseline report does not parse: {error}"),
        )
    })?;
    if baseline.status != ReportStatus::Ok {
        return Err(BenchError::new(
            BenchCode::BaselineInvalid,
            format!(
                "the baseline report {} did not finish (status failed)",
                baseline.variant.label
            ),
        ));
    }
    Ok(baseline)
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsistencySection {
    pub baseline_label: String,
    pub roi: RoiConsistency,
    pub full_frame: FullFrameConsistency,
    pub requests: Vec<RequestConsistency>,
    pub gate: GateVerdict,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RoiConsistency {
    pub total: usize,
    pub text_equal: usize,
    /// Requests the baseline read as their truth text and this run did not.
    pub baseline_right_this_wrong: Vec<String>,
    pub max_confidence_delta_milli: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FullFrameConsistency {
    pub total: usize,
    pub box_count_equal: usize,
    pub text_set_equal: usize,
    pub matched_boxes: usize,
    pub unmatched_boxes: usize,
    pub min_iou_milli: Option<u32>,
    pub mean_iou_milli: Option<u32>,
    pub max_confidence_delta_milli: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestConsistency {
    pub frame_id: String,
    pub request_id: String,
    pub kind: RequestKind,
    pub text_equal: bool,
    pub confidence_delta_milli: Option<u32>,
    pub baseline_boxes: usize,
    pub boxes: usize,
    pub text_set_equal: bool,
    pub matched_boxes: usize,
    pub min_iou_milli: Option<u32>,
    pub mean_iou_milli: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateVerdict {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// Compares this run's requests with the baseline's, which must hold exactly the same requests.
pub fn compare(
    baseline: &BaselineReport,
    requests: &[RequestRecord],
) -> Result<ConsistencySection, BenchError> {
    let mut by_id = BTreeMap::new();
    for record in &baseline.engine.requests {
        by_id.insert(
            (record.frame_id.as_str(), record.request_id.as_str()),
            record,
        );
    }
    if by_id.len() != requests.len() {
        return Err(BenchError::new(
            BenchCode::BaselineInvalid,
            format!(
                "the baseline holds {} requests, this run {}",
                by_id.len(),
                requests.len()
            ),
        ));
    }
    let mut roi = RoiConsistency::default();
    let mut full = FullFrameConsistency::default();
    let mut iou_sum: u64 = 0;
    let mut rows = Vec::new();
    for record in requests {
        let Some(base) = by_id.get(&(record.frame_id.as_str(), record.request_id.as_str())) else {
            return Err(BenchError::new(
                BenchCode::BaselineInvalid,
                format!(
                    "the baseline lacks request {} of frame {}",
                    record.request_id, record.frame_id
                ),
            ));
        };
        let text_equal = base.output.text == record.output.text;
        let matching = match_blocks(&base.output.blocks, &record.output.blocks);
        let text_set_equal = text_set(&base.output) == text_set(&record.output);
        let confidence_delta_milli = match record.kind {
            RequestKind::Roi => delta_milli(base.output.confidence, record.output.confidence),
            RequestKind::FullFrame => matching.max_confidence_delta_milli,
        };
        match record.kind {
            RequestKind::Roi => {
                roi.total += 1;
                if text_equal {
                    roi.text_equal += 1;
                }
                if base.matches_truth == Some(true) && record.matches_truth != Some(true) {
                    roi.baseline_right_this_wrong
                        .push(format!("{}/{}", record.frame_id, record.request_id));
                }
                roi.max_confidence_delta_milli = roi
                    .max_confidence_delta_milli
                    .max(confidence_delta_milli.unwrap_or(0));
            }
            RequestKind::FullFrame => {
                full.total += 1;
                let base_count = base
                    .output
                    .detected_boxes
                    .map_or(base.output.blocks.len(), |count| count as usize);
                let count = record
                    .output
                    .detected_boxes
                    .map_or(record.output.blocks.len(), |count| count as usize);
                if base_count == count {
                    full.box_count_equal += 1;
                }
                if text_set_equal {
                    full.text_set_equal += 1;
                }
                full.matched_boxes += matching.ious.len();
                full.unmatched_boxes += (base.output.blocks.len() - matching.ious.len())
                    + (record.output.blocks.len() - matching.ious.len());
                for iou in &matching.ious {
                    iou_sum += u64::from(*iou);
                    full.min_iou_milli = Some(full.min_iou_milli.map_or(*iou, |min| min.min(*iou)));
                }
                full.max_confidence_delta_milli = full
                    .max_confidence_delta_milli
                    .max(confidence_delta_milli.unwrap_or(0));
            }
        }
        rows.push(RequestConsistency {
            frame_id: record.frame_id.clone(),
            request_id: record.request_id.clone(),
            kind: record.kind,
            text_equal,
            confidence_delta_milli,
            baseline_boxes: base.output.blocks.len(),
            boxes: record.output.blocks.len(),
            text_set_equal,
            matched_boxes: matching.ious.len(),
            min_iou_milli: matching.ious.iter().copied().min(),
            mean_iou_milli: mean(&matching.ious),
        });
    }
    if full.matched_boxes > 0 {
        full.mean_iou_milli = u32::try_from(iou_sum / full.matched_boxes as u64).ok();
    }
    let gate = gate(&roi, &full);
    Ok(ConsistencySection {
        baseline_label: baseline.variant.label.clone(),
        roi,
        full_frame: full,
        requests: rows,
        gate,
    })
}

fn gate(roi: &RoiConsistency, full: &FullFrameConsistency) -> GateVerdict {
    let mut reasons = Vec::new();
    if roi.total - roi.text_equal > GATE_ROI_TEXT_DIFFERENCES {
        reasons.push(format!(
            "ROI texts equal {}/{}, fewer than {}/{}",
            roi.text_equal,
            roi.total,
            roi.total.saturating_sub(GATE_ROI_TEXT_DIFFERENCES),
            roi.total
        ));
    }
    if !roi.baseline_right_this_wrong.is_empty() {
        reasons.push(format!(
            "the baseline read the truth and this run did not: {}",
            roi.baseline_right_this_wrong.join(", ")
        ));
    }
    if full.total - full.box_count_equal > GATE_BOX_COUNT_DIFFERENCES {
        reasons.push(format!(
            "frames with the same box count {}/{}, fewer than {}/{}",
            full.box_count_equal,
            full.total,
            full.total.saturating_sub(GATE_BOX_COUNT_DIFFERENCES),
            full.total
        ));
    }
    if full.text_set_equal != full.total {
        reasons.push(format!(
            "frames with the same text set {}/{}",
            full.text_set_equal, full.total
        ));
    }
    let delta = roi
        .max_confidence_delta_milli
        .max(full.max_confidence_delta_milli);
    if delta > GATE_CONFIDENCE_DELTA_MILLI {
        reasons.push(format!(
            "a confidence differs by {delta} milli, more than {GATE_CONFIDENCE_DELTA_MILLI}"
        ));
    }
    GateVerdict {
        passed: reasons.is_empty(),
        reasons,
    }
}

struct BlockMatching {
    ious: Vec<u32>,
    max_confidence_delta_milli: Option<u32>,
}

/// Pairs each block with at most one block of the other reading, best IoU first.
fn match_blocks(baseline: &[BlockOutput], candidate: &[BlockOutput]) -> BlockMatching {
    let mut pairs = Vec::new();
    for (left_index, left) in baseline.iter().enumerate() {
        for (right_index, right) in candidate.iter().enumerate() {
            let iou = iou_milli(left.rect, right.rect);
            if iou > 0 {
                pairs.push((iou, left_index, right_index));
            }
        }
    }
    pairs.sort_unstable_by_key(|pair| std::cmp::Reverse(*pair));
    let mut left_used = vec![false; baseline.len()];
    let mut right_used = vec![false; candidate.len()];
    let mut ious = Vec::new();
    let mut max_delta: Option<u32> = None;
    for (iou, left_index, right_index) in pairs {
        if left_used[left_index] || right_used[right_index] {
            continue;
        }
        left_used[left_index] = true;
        right_used[right_index] = true;
        ious.push(iou);
        if let Some(delta) = delta_milli(
            baseline[left_index].confidence,
            candidate[right_index].confidence,
        ) {
            max_delta = Some(max_delta.map_or(delta, |max| max.max(delta)));
        }
    }
    BlockMatching {
        ious,
        max_confidence_delta_milli: max_delta,
    }
}

/// Intersection over union, in milli (rounded down).
pub fn iou_milli(left: WorkloadRect, right: WorkloadRect) -> u32 {
    let area = |rect: WorkloadRect| i64::from(rect.width.max(0)) * i64::from(rect.height.max(0));
    let x0 = i64::from(left.x.max(right.x));
    let y0 = i64::from(left.y.max(right.y));
    let x1 = (i64::from(left.x) + i64::from(left.width))
        .min(i64::from(right.x) + i64::from(right.width));
    let y1 = (i64::from(left.y) + i64::from(left.height))
        .min(i64::from(right.y) + i64::from(right.height));
    let intersection = (x1 - x0).max(0) * (y1 - y0).max(0);
    let union = area(left) + area(right) - intersection;
    if union <= 0 {
        return 0;
    }
    u32::try_from(intersection * 1_000 / union).unwrap_or(0)
}

fn delta_milli(left: Option<f32>, right: Option<f32>) -> Option<u32> {
    let (Some(left), Some(right)) = (left, right) else {
        return None;
    };
    let delta = (f64::from(left) - f64::from(right)).abs() * 1_000.0;
    Some(delta.round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

fn text_set(output: &RequestOutput) -> Vec<&str> {
    let mut texts: Vec<&str> = output
        .blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect();
    texts.sort_unstable();
    texts
}

fn mean(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let sum: u64 = values.iter().map(|value| u64::from(*value)).sum();
    u32::try_from(sum / values.len() as u64).ok()
}
