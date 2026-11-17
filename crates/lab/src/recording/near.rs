// SPDX-License-Identifier: AGPL-3.0-only

//! #463 RM-1 (`contracts/lab-recording.md`, "Self-checks", item 11, "Near a threshold"): a template
//! mark whose score lies nearer its threshold than a bound, on the side a self-check of `record
//! stop` needs it on. The self-checks already evaluate every template mark with the admitted
//! evaluator on the frames they judge; they keep, per mark and per judgement, the frame on which its
//! score came nearest its threshold ([`Nearest`]), and a mark nearer than the bound is the warning
//! `record_template_near_threshold` ([`TemplateNearThreshold`]): the fact only, with the mark, where
//! the judgement is, the score, the threshold, the margin and the bound. It refuses nothing,
//! proposes no mark, region or step, and is never written into the package, so that the package is
//! the same whatever the bound.
//!
//! The bound is a parameter of the commands that run the self-checks, as the Lab's other
//! self-check numbers are (`--arrival-timeout-ms` of `record stop`, `--before` and `--after` of
//! `record frames`, a mark's `--threshold`): `--template-margin-bound` of `record stop`, `record
//! ready` and `record review` (`RecordStopOptions::template_margin_bound`), its default
//! [`DEFAULT_TEMPLATE_MARGIN_BOUND`].

use super::model::RecordedFrame;
use super::store::{invalid, with_details};
use crate::codes::{LabCode, TemplateThresholdSide};
use actingcommand_contract::LabResult;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

/// The bound of `--template-margin-bound` when it is not given.
pub(crate) const DEFAULT_TEMPLATE_MARGIN_BOUND: f64 = 0.03;

/// The name of the option the bound is given by, as a refusal names it.
const BOUND_OPTION: &str = "--template-margin-bound";

/// The bound the self-checks warn by: `given`, or [`DEFAULT_TEMPLATE_MARGIN_BOUND`]; a bound that is
/// not a finite number from 0 to 1 is `validation_failed` (`details{option, value}`), before
/// anything is read.
pub(crate) fn template_margin_bound(given: Option<f64>) -> LabResult<f64> {
    let Some(bound) = given else {
        return Ok(DEFAULT_TEMPLATE_MARGIN_BOUND);
    };
    if !bound.is_finite() || !(0.0..=1.0).contains(&bound) {
        return Err(with_details(
            invalid(
                "validation_failed",
                format!("{BOUND_OPTION} must be a finite number from 0 to 1"),
            ),
            json!({"option": BOUND_OPTION, "value": bound.to_string()}),
        ));
    }
    Ok(bound)
}

/// A template mark's score and threshold on one frame, as the admitted evaluator gave them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TemplateScore {
    pub(crate) score: f32,
    pub(crate) threshold: f32,
}

impl TemplateScore {
    /// Whether the score lies at or above the threshold.
    pub(crate) fn at_or_above(self) -> bool {
        self.score >= self.threshold
    }

    /// How far the score lies from the threshold, on whichever side.
    fn distance(self) -> f64 {
        (f64::from(self.score) - f64::from(self.threshold)).abs()
    }
}

/// Where the judgement of a warning is, in the members the check it stands for names it by: a
/// step's page by the step's number in the package (`step`) and its serial number
/// (`record_index`, the one `record mark --step` and `record review` take), an arrival gate by the
/// step after whose effect it is awaited and the page awaited (`gate`), an optional step's page by
/// the screen it is judged against (`against`), the skip target by the optional step on whose
/// frames it is judged (`optional_step`), an ending's page by its outcome key (and the other
/// screen's `step` or `other_outcome_key`), the usual ending's page on an ending's frames by
/// `usual_page`. Every member a judgement does not name is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NearThresholdAt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub against: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional_step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_outcome_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usual_page: Option<String>,
}

/// The warning `record_template_near_threshold`: the template mark `mark_id` came nearest its
/// threshold on the frame `frame_id` of the frames the check `check` judges it on, on the side
/// `side` that check needs, with the score `score` and the threshold `threshold` the admitted
/// evaluator gave there, `margin` how far the one lies from the other (`score − threshold` for
/// `passes`, `threshold − score` for `misses`), less than `bound`. `check` names the check by the
/// code it answers when the judgement fails, as the warnings of a check that was not evaluated
/// name it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemplateNearThreshold {
    /// `record_template_near_threshold`.
    pub code: String,
    /// A word of `template_threshold_side`: `passes` or `misses`.
    pub side: String,
    pub check: String,
    #[serde(flatten)]
    pub at: NearThresholdAt,
    pub mark_id: String,
    pub frame_id: String,
    pub score: f64,
    pub threshold: f64,
    pub margin: f64,
    pub bound: f64,
}

/// Per template mark, the frame of one judgement on which its score came nearest its threshold,
/// and that score; the first such frame when two come as near.
#[derive(Debug, Default)]
pub(crate) struct Nearest(BTreeMap<String, (String, TemplateScore)>);

impl Nearest {
    /// The mark `mark_id` scored `score` on `frame`; a score that is no number is no distance and
    /// is not kept.
    pub(crate) fn note(&mut self, mark_id: &str, frame: &RecordedFrame, score: TemplateScore) {
        if !score.distance().is_finite() {
            return;
        }
        let nearer = self
            .0
            .get(mark_id)
            .is_none_or(|(_, kept)| score.distance() < kept.distance());
        if nearer {
            self.0
                .insert(mark_id.to_owned(), (frame.frame_id.clone(), score));
        }
    }

    /// The warnings of the marks noted whose nearest score lies within `bound` of its threshold,
    /// on the side `side` the check `check` at `at` needs; none for a bound of 0. In the order of
    /// the marks' ids.
    pub(crate) fn within(
        &self,
        bound: f64,
        side: TemplateThresholdSide,
        check: &str,
        at: &NearThresholdAt,
    ) -> Vec<TemplateNearThreshold> {
        self.0
            .iter()
            .filter(|(_, (_, score))| score.distance() < bound)
            .map(|(mark_id, (frame_id, score))| TemplateNearThreshold {
                code: LabCode::RecordTemplateNearThreshold.as_str().to_owned(),
                side: side.as_str().to_owned(),
                check: check.to_owned(),
                at: at.clone(),
                mark_id: mark_id.clone(),
                frame_id: frame_id.clone(),
                score: f64::from(score.score),
                threshold: f64::from(score.threshold),
                margin: score.distance(),
                bound,
            })
            .collect()
    }
}
