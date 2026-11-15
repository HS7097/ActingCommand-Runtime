// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab recording state (`actingcommand.lab-recording.v1`), the mark request
//! (`actingcommand.lab-record-mark.v1`) and the outcome views the CLI prints.

use actingcommand_contract::{
    InputFrameReference, InputSamplingEvidence, LabError, LabResult, OcrUnsignedIntegerFormat,
    OcrValueTrim, SEGMENTED_SWIPE_BRAKE_DURATION_MS, SEGMENTED_SWIPE_CORNER_HOLD_MS,
    SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS, ValueTestOp,
};
use actingcommand_execution_kernel::DriveDraw;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const LAB_RECORDING_SCHEMA: &str = "actingcommand.lab-recording.v1";
pub const LAB_RECORD_MARK_SCHEMA: &str = "actingcommand.lab-record-mark.v1";
pub const LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD: f64 = 0.95;
pub const LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE: u32 = 20;
pub const LAB_RECORDING_DEFAULT_MATCH_METRIC: &str = "ccoeff_normed";
/// The settle of `record mark --optional` without `--settle-ms` on a step not optional yet.
pub const LAB_RECORDING_DEFAULT_SETTLE_MS: u64 = 2_000;
/// The largest settle of an optional step (the package's largest step timeout).
pub const LAB_RECORDING_MAX_SETTLE_MS: u64 = 60_000;
/// #463 L1: the longest wait between a one-call step's performed press and its next frame, the
/// bound a settle has.
pub const LAB_RECORDING_MAX_NEXT_FRAME_MS: u64 = LAB_RECORDING_MAX_SETTLE_MS;
/// Workflow #390 LAB-2: the largest duration of a recorded drag or long press (the Runtime's
/// largest input duration).
pub const LAB_RECORDING_MAX_GESTURE_MS: u64 = 60_000;
/// Workflow #390 LAB-2: a template mark without a declared search area searches its crop grown by
/// this many pixels on each side, clamped to the frame. A search area equal to the crop fails as
/// soon as the control moves at all (a game update once shifted a menu bar by about 40 px).
pub const LAB_RECORDING_DEFAULT_SEARCH_MARGIN: i32 = 48;

/// Opaque JSON carried through the recording as recorded (Runtime references, freshness,
/// operation evidence). The recording never interprets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueJson(serde_json::Value);

impl OpaqueJson {
    pub fn from_serializable<T: Serialize>(value: &T) -> LabResult<Self> {
        serde_json::to_value(value)
            .map(Self)
            .map_err(|error| LabError::usage(format!("failed to encode recording value: {error}")))
    }

    /// Workflow #449 REC-ARCH: the value as recorded, which an archive's manifest copies
    /// member by member where the step table has it.
    pub(crate) fn value(&self) -> &serde_json::Value {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingDefaults {
    pub template_threshold: f64,
    pub color_max_distance: u32,
    pub match_metric: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabRecording {
    pub schema_version: String,
    pub record_id: String,
    pub record_started_at_unix_ms: u64,
    pub task_id: String,
    pub instance: String,
    pub status: String,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub defaults: RecordingDefaults,
    pub coordinate_space: Option<RecordSize>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub steps: Vec<RecordingStep>,
    pub artifact: Option<RecordingArtifact>,
    /// Workflow #390 LAB-2: when a stopped recording was last marked again after its package was
    /// generated; the next `record stop` generates the package again and clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amended_at_unix_ms: Option<u64>,
    /// #463 R2: the endings a stopped recording took after its last press, each from a frame, in
    /// the order they were taken; absent on a recording without one, so that it is byte-identical
    /// to one an earlier build wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endings: Vec<RecordingEnding>,
}

/// #463 R2 (`contracts/lab-recording.md`, "Endings"): a screen that ends the business another way
/// after the last press, taken by a stopped recording from a frame. It is no step: `record stop`
/// generates its page (`ending_kk` or `ending_kk_<page>`), whose rule requires its marks, and its
/// `scheduling_outcome` mapping (`outcome_key` on that page). Its marks carry `step` 0, the number
/// of no step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingEnding {
    /// The key of the run's end on this screen.
    pub outcome_key: String,
    /// The name its page takes, `ending_kk_<page>`; `None` names it `ending_kk`.
    pub page: Option<String>,
    /// Its primary frame and samples, stored in the recording as a step's are.
    pub frames: Vec<RecordedFrame>,
    /// Its own marks, self-tested on its frames.
    pub marks: Vec<RecordedMark>,
    /// The marks of steps its page requires too, self-tested on its frames.
    pub reused: Vec<String>,
    pub marked_at_unix_ms: u64,
    /// Workflow #434 LAB-2: the initiator word of the process that took it.
    pub initiator: String,
}

impl RecordingEnding {
    /// Its live frames: the primary frame and the samples.
    pub fn live_frames(&self) -> Vec<&RecordedFrame> {
        self.frames
            .iter()
            .filter(|frame| !frame.superseded)
            .collect()
    }
}

/// #463 R2: the `ending` of a `record mark` request on a stopped recording: its frame and samples
/// (files on the caller's machine, copied into the recording), its page's name, its outcome key and
/// its marks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndingSpec {
    pub frame: String,
    #[serde(default)]
    pub samples: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    pub outcome_key: String,
    #[serde(default)]
    pub add: Vec<MarkSpec>,
    #[serde(default)]
    pub reuse: Vec<String>,
}

/// Written by `record stop` (#336 L4): the package generated from the recording. Workflow #336
/// PKG-S5 (option 丙): the package is named by its `package_id`; no digest of it is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingArtifact {
    /// `zip` or `json`.
    pub container: String,
    /// The copy inside the recording, `lab/out/<package_id>.<container>`.
    pub path: String,
    pub lab_dir_path: Option<String>,
    pub byte_count: u64,
    pub package_id: String,
    /// The prerequisite package id written into control.json (`--requires`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
    /// The game, server and locale the package was generated with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// The task timeout, arrival timeout and application arrival timeout the package was
    /// generated with (defaults applied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_arrival_timeout_ms: Option<u64>,
    pub generated_at_unix_ms: u64,
}

/// `record stop` options (Workflow #336 L4). Game, server and locale are canonical; the
/// explicit ones win over the recording's, which win over the `default_*` ones (the
/// instance configuration).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordStopOptions {
    pub lab_dir: Option<String>,
    pub package_id: Option<String>,
    pub requires: Option<String>,
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub default_game: Option<String>,
    pub default_server: Option<String>,
    pub timeout_ms: Option<u64>,
    pub arrival_timeout_ms: Option<u64>,
    pub application_arrival_timeout_ms: Option<u64>,
    pub dry_run: bool,
}

/// The Lab part of `record stop`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordStopOutcome {
    /// `generated`, `validated` or `already_generated`; `None` when the session has no Lab
    /// steps and `record stop` behaves as before.
    pub lab_status: Option<String>,
    /// The `lab` output field; `None` prints `null`.
    pub lab: Option<OpaqueJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingStep {
    pub index: u32,
    pub page: Option<String>,
    pub dropped: bool,
    pub converted_to_transition: bool,
    pub frames: Vec<RecordedFrame>,
    pub marks: Vec<RecordedMark>,
    pub reused: Vec<String>,
    pub click: Option<StepClick>,
    pub click_guard: Option<String>,
    /// The application operation of the step (R24); a step has a click or this, not both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<StepApplication>,
    /// The step's page may not appear; the package then skips the step (Workflow #339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<StepOptional>,
    /// Workflow #390 LAB-2: marks of other steps that must not pass on this step's page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    /// Workflow #390 LAB-2: the own template mark that anchors this step's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// #463 R1: the step chooses in a list of fixed slots; its click (`source: "select"`) is the
    /// slot the decision selects for the pressed name on the primary frame. Absent on every other
    /// step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<StepSelect>,
    pub transition: Option<StepTransition>,
    pub closed: bool,
    pub closed_by: Option<String>,
    /// Workflow #434 LAB-2 (`contracts/lab-two-entries.md` §4.2, §4.3 "Recordings"): the initiator
    /// word of the process that added the step (`Lab-CLI`, `Lab-制包` or `Lab-救急`).
    pub initiator: String,
}

/// An optional step (Workflow #339): after the page that follows its run of optional steps
/// is first seen, the package keeps watching for a late optional page for `settle_ms`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepOptional {
    pub settle_ms: u64,
    pub marked_at_unix_ms: u64,
}

impl RecordingStep {
    /// Neither dropped nor converted into the transition of the previous step.
    pub fn is_effective(&self) -> bool {
        !self.dropped && !self.converted_to_transition
    }

    /// The application entry step: an application operation and no frame (`from:"any"`).
    pub fn is_application_entry(&self) -> bool {
        self.application.is_some() && self.frames.is_empty()
    }
}

/// The application operation of a step (Workflow #336 R24): `launch`, `restart` or `stop` of
/// the instance's assigned application, executed by `session app … --record` or declared by
/// `record mark --application`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepApplication {
    /// `launch`, `restart` or `stop` (`force-stop` is recorded as `stop`).
    pub action: String,
    /// The verb the command used (`force-stop` stays `force-stop` here).
    pub cli_verb: String,
    /// `executed` (a completed receipt is on record) or `declared`.
    pub source: String,
    pub executed: Option<ApplicationExecution>,
    pub attempts: Vec<ApplicationAttempt>,
    pub needs_review: bool,
}

/// The completed receipt of the application operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationExecution {
    pub request_id: String,
    pub correlation_id: String,
    pub action_id: String,
    pub receipt_state: String,
    pub application_event_ids: Vec<String>,
}

/// An application operation without a completed receipt (it may have run), or a completed
/// one moved aside by `--reopen-step`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationAttempt {
    /// The receipt state (`failed`, `completed`, …) or `none` without a receipt.
    pub receipt_state: String,
    pub runtime_code: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedFrame {
    pub frame_id: String,
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub byte_count: u64,
    pub source: String,
    pub runtime_artifact: Option<OpaqueJson>,
    pub capture_backend: Option<String>,
    pub freshness: Option<OpaqueJson>,
    pub recorded_at_unix_ms: u64,
    pub superseded: bool,
    /// #463 R2 fix 1: the size an ending's frame was given at, when the Lab converted it to the
    /// recording's size as the Runtime converts a frame of a pure scale (`frames.rs`,
    /// `scaled_to_recording`); absent on every other frame, so that a recording without one is
    /// byte-identical to one an earlier build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaled_from: Option<RecordSize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkFamily {
    Template,
    Color,
    ColorDigest,
    Ocr,
    Check,
    /// #411 Lab B-9: a value test whose sides are inline OCR reads (`value_test`).
    ValueTest,
}

impl MarkFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Color => "color",
            Self::ColorDigest => "color_digest",
            Self::Ocr => "ocr",
            Self::Check => "check",
            Self::ValueTest => "value_test",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfTestStatus {
    Passed,
    Failed,
    NotEvaluated,
}

impl SelfTestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NotEvaluated => "not_evaluated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelfTestMargin {
    Digest {
        mean_milli: i64,
        max_cell: Option<i64>,
    },
    Value(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfTest {
    pub status: SelfTestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub frames: u32,
    pub single_sample: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<SelfTestMargin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_rect: Option<RecordRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_ambiguous: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_observed_distance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_milli: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cell: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<BTreeMap<String, SelfTestStatus>>,
    /// Workflow #396 REC-4: the declared factors of a template mark with `scales`, in their
    /// declared order, that its template passed at on at least one frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_scales_milli: Option<Vec<u32>>,
}

/// Workflow #396 REC-4 (REC-5): the factors a template mark is resampled by and searched at, as
/// the task's `verify_templates[].scales` declares them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateScalesSpec {
    pub factors_milli: Vec<u32>,
}

/// Workflow #396 REC-4 (REC-7): the gray range of the template pixels a template mark scores,
/// as the task's `verify_templates[].mask` declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateMaskSpec {
    Range { lower: u8, upper: u8 },
}

/// Workflow #396 REC-4 (REC-2): a second place within this margin of the best match fails a
/// template mark, as the task's `verify_templates[].unique` declares it; #463 L2: and a second
/// matching block fails a located-text mark, as the task's `ocr_targets[].unique` declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateUniqueSpec {
    pub margin_milli: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkCrop {
    pub path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub asset: String,
}

/// The OCR declaration fields of a task 0.9 `ocr_targets[]` entry; all are required, `locate`
/// is the one optional member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrSpec {
    pub languages: Vec<String>,
    pub timeout_ms: u64,
    pub match_mode: String,
    pub expected: Vec<String>,
    pub case_sensitive: bool,
    pub minimum_confidence: f64,
    pub model_ref: String,
    pub model_sha256: String,
    /// #463 L2: a located-text mark finds one of its `expected` names block by block, as the
    /// task's `ocr_targets[].locate` declares it (`contracts/ocr-fields.md`, "Located text"); a
    /// click may follow the box it finds. Absent on every other OCR mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locate: Option<OcrLocateSpec>,
}

/// #463 L2: the `locate` of a located-text mark, member for member as the task's
/// `ocr_targets[].locate` declares it; every member is the author's, with no Lab default. The
/// Runtime's declaration rules (a `line_merge` exactly with `detect`, the cell grid inside the
/// region, 1 to 64 cells) are applied when the mark is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocateSpec {
    pub normalize: OcrLocateNormalizeSpec,
    pub blocks: OcrLocateBlocksSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_merge: Option<OcrLocateLineMergeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<OcrLocateCellsSpec>,
}

/// #463 L2: the normalization of blocks and `expected` names (`identity_v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OcrLocateNormalizeSpec {
    #[serde(rename = "identity_v1")]
    IdentityV1,
}

/// #463 L2: how the provider cuts the region into blocks (`detect` or `region`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrLocateBlocksSpec {
    Detect,
    Region,
}

/// #463 L2: `"none"` or `{"gap_factor": n}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OcrLocateLineMergeSpec {
    Keyword(OcrLocateLineMergeKeywordSpec),
    Factor(OcrLocateGapFactorSpec),
}

/// #463 L2: the line merge keyword `none`, which merges no boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OcrLocateLineMergeKeywordSpec {
    #[serde(rename = "none")]
    NoMerge,
}

/// #463 L2: the folder's line merge with this factor in place of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocateGapFactorSpec {
    pub gap_factor: u32,
}

/// #463 L2: a grid of cells relative to the region's top-left corner, cell
/// `k = row * columns + column`; the click box of a following click is the cell holding the found
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrLocateCellsSpec {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub pitch_x: i64,
    pub pitch_y: i64,
    pub columns: i64,
    pub rows: i64,
}

/// #411 Lab B-9: one side of a value-test mark, an inline OCR read. The package declares it as
/// the OCR target `<mark id>/left` or `<mark id>/right` with the region, languages, timeout,
/// minimum confidence and model, and the value test reads that target's text with `trim`,
/// `value` and `part`. `mul` and `add` are declared on the right side only, and there both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTestSide {
    pub region: RecordRect,
    pub languages: Vec<String>,
    pub timeout_ms: u64,
    pub minimum_confidence: f64,
    pub model_ref: String,
    pub model_sha256: String,
    pub trim: OcrValueTrim,
    pub value: ValueTestValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<ValueTestPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mul: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<i64>,
}

/// #411 Lab B-9: the value a side reads; without `format` the Runtime parses `ascii_decimal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueTestValue {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<OcrUnsignedIntegerFormat>,
    },
}

/// #411 Lab B-9: which part of a `current_capacity` text a side reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestPart {
    Current,
    Capacity,
}

/// #411 Lab B-9: the right side `{"const": n}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTestConst {
    #[serde(rename = "const")]
    pub constant: u64,
}

/// #411 Lab B-9: the right side of a comparison: a constant, or another side with `mul` and
/// `add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueTestRight {
    Const(ValueTestConst),
    Side(Box<ValueTestSide>),
}

/// #411 Lab B-9: a value-test mark as declared (`recording.json` `value_test`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTestSpec {
    pub left: ValueTestSide,
    pub op: ValueTestOp,
    /// Absent exactly for `parses`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<ValueTestRight>,
}

impl ValueTestSpec {
    /// The OCR targets the package declares for the sides of the value test `id`, as
    /// `(target id, side)`: `<id>/left`, and `<id>/right` when the right side is read.
    pub fn sides(&self, id: &str) -> Vec<(String, &ValueTestSide)> {
        let mut sides = vec![(value_test_side_id(id, "left"), &self.left)];
        if let Some(ValueTestRight::Side(right)) = &self.right {
            sides.push((value_test_side_id(id, "right"), right.as_ref()));
        }
        sides
    }
}

/// #411 Lab B-9: the id of a value test's side target, `<id>/left` or `<id>/right`.
pub(crate) fn value_test_side_id(id: &str, side: &str) -> String {
    format!("{id}/{side}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedMark {
    pub id: String,
    pub family: MarkFamily,
    pub step: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_of: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<RecordRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<RecordRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<MarkCrop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_distance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_cells: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mean_milli: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cell: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<OcrSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<String>>,
    /// Workflow #396 REC-4: a template mark's factors (REC-5), mask (REC-7) and margin of
    /// uniqueness (REC-2), as declared; #463 L2: a located-text mark's margin of uniqueness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scales: Option<TemplateScalesSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<TemplateMaskSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique: Option<TemplateUniqueSpec>,
    /// #411 Lab B-9: a value-test mark's declaration; such a mark has no `region`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_test: Option<ValueTestSpec>,
    pub self_test: SelfTest,
    pub created_at_unix_ms: u64,
    /// Workflow #434 LAB-2 (`contracts/lab-two-entries.md` §4.2, §4.3 "Recordings"): the initiator
    /// word of the process that added the mark.
    pub initiator: String,
}

impl RecordedMark {
    pub fn check_members(&self) -> &[String] {
        self.all_of
            .as_deref()
            .or(self.any_of.as_deref())
            .unwrap_or(&[])
    }

    /// #411 Lab B-9: every target id the package declares for the mark: its own id and, for a
    /// value test, the ids of its side targets.
    pub fn target_ids(&self) -> Vec<String> {
        let mut ids = vec![self.id.clone()];
        if let Some(test) = &self.value_test {
            ids.extend(test.sides(&self.id).into_iter().map(|(id, _)| id));
        }
        ids
    }

    /// Workflow #396 REC-4: a template mark that declares factors, a mask or uniqueness. Its
    /// self-test evaluates it as the Runtime evaluates the target the package declares for it.
    pub fn declares_exact_template(&self) -> bool {
        self.scales.is_some() || self.mask.is_some() || self.unique.is_some()
    }

    /// #463 L2: an OCR mark that declares `locate`, which a click may follow.
    pub fn locates_text(&self) -> bool {
        self.family == MarkFamily::Ocr && self.ocr.as_ref().is_some_and(|ocr| ocr.locate.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickRetry {
    pub max_attempts: u32,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickExecution {
    pub point: RecordPoint,
    pub point_rule: String,
    pub effect: String,
    pub carrier_package: Option<OpaqueJson>,
    pub req_id: Option<OpaqueJson>,
    pub correlation_id: Option<OpaqueJson>,
    pub action_id: Option<OpaqueJson>,
    pub lease_id: Option<OpaqueJson>,
    pub failure: Option<OpaqueJson>,
    pub before: Option<OpaqueJson>,
    pub after: Option<OpaqueJson>,
    pub executed_at_unix_ms: u64,
    /// Workflow #390 LAB-2: where an executed drag or long press ended (a long press ends where
    /// it began) and how long it took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_point: Option<RecordPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Workflow #406 T4: how the Lab drew the pressed points when it was given a seed (the
    /// evidence of `point_rule`'s draw: its algorithm, action seed, boxes and parameters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<InputSamplingEvidence>,
    /// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): on a press recorded by
    /// the command that makes it (`tap`, `swipe` or `long-tap --record`), the id of the step's live
    /// primary frame when the press was committed: what was on the screen. Absent on every other
    /// press, as are the two members below, so that a recording without such a press is
    /// byte-identical to one an earlier build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_frame: Option<String>,
    /// #463 S1-1: on such a press, the id of the first frame that arrived after it: what the
    /// screen became. #463 S1-2: the last capture of the press's own after loop, written once when
    /// the loop ends (fix round 3: also when a later capture failed); the first frame that opens a
    /// step after it only when the loop kept nothing (its first capture could not be taken).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_frame: Option<String>,
    /// #463 S1-1: on such a press, how the frame after it was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_capture: Option<AfterCaptureRecord>,
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): how the frame after a
/// press recorded by the command that makes it was taken. #463 S1-2: the screen is captured until
/// it is still, and these are the parameters as the press took them and what the loop did: the
/// wait before the first capture (`after_ms`), the time from the start of one capture to the start
/// of the next (`interval_ms`), how long the screen must not change to be still (`still_ms`; 0: no
/// stillness test), the bound after the first capture (`still_max_ms`), whether the screen was
/// found still (`still`: `true`, `false` at the bound, `None` when no test ran or the loop did not
/// finish), the time from the press to the start of the last capture (`waited_ms`), how many
/// captures it took (`captures`) and how many of them it kept beside the last, the frame after
/// (`kept`, `captures - 1`). Fix round 3: `unfinished` when a capture after the first could not be
/// taken, so that the loop ended there and kept what it had taken (absent otherwise). A record
/// without `interval_ms` and `still_ms` (S1-1's build wrote one capture after the wait) reads as
/// one whose screen after was not tested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AfterCaptureRecord {
    pub after_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_ms: Option<u64>,
    pub still_max_ms: u64,
    pub still: Option<bool>,
    pub waited_ms: u64,
    pub captures: u32,
    pub kept: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unfinished: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepClick {
    /// The click rectangle on the primary frame; a click that follows a template is executed
    /// and shown with it while recording.
    pub rect: RecordRect,
    pub source: String,
    pub from: Option<String>,
    /// Workflow #396 REC-4: the template mark the package's click follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<ClickFollow>,
    /// Workflow #390 LAB-2: a drag from `rect` or a long press at `rect`'s one point instead of a
    /// tap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gesture: Option<ClickGesture>,
    pub retry: Option<ClickRetry>,
    pub declared_at_unix_ms: u64,
    pub execution: Option<ClickExecution>,
    pub attempts: Vec<ClickExecution>,
    pub needs_review: bool,
}

/// Workflow #396 REC-4: how the package's click follows a template mark. `target` clicks the
/// match box; `offset` clicks the match box's top-left corner plus `offset`, which is the
/// declared rectangle minus the crop's top-left corner (scaled by the factor the template
/// matched at, for a template with `scales`). #463 L2: or a located-text mark: its box is the
/// found block, or the declared cell holding it, and `offset` is the declared rectangle minus that
/// box's top-left corner on the primary frame, fixed when the click is declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickFollow {
    pub mark_id: String,
    pub form: ClickFollowForm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<RecordRect>,
    /// #463 L2 fix 1: the `expected` names of the followed located-text mark were changed after
    /// the step's click was executed, so the recorded press is not what the package presses: the
    /// package presses where the new names are found. Absent (false) on every other click.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub names_changed_after_press: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickFollowForm {
    Target,
    Offset,
}

/// Workflow #390 LAB-2: the gesture of a step whose effect is not a tap. A `drag` presses a
/// point of the step's click rectangle, moves to a point of `to` and lifts there after
/// `duration_ms` (a fast drag is a swipe); the package declares it as a `drag` click, whose two
/// rectangles the kernel samples like a tap's. A `long_press` holds a point of the step's click
/// rectangle for `duration_ms`; the package declares it as a `long_press` click with that box
/// (Workflow #390 lpbox), in which the kernel picks the point it holds. #411 Lab BD: a
/// `single_touch_drag_with_vertical_brake_v1` presses a point of the step's click rectangle,
/// moves to a point of the box `corner`, holds there and brakes 100 px up before it lifts, with
/// the Runtime's fixed timings; the package declares it as that click, and a Lab operation does
/// not press it. Workflow #406 T4: a `single_touch_path_drag_v1` turns `turns` times inside the
/// step's click rectangle (its box) in one touch, each segment at least `min_segment_px` long,
/// over `duration_ms` with `corner_hold_ms` at each inner corner; the package declares it as
/// that click, the kernel generates its path, and a Lab operation does not press it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClickGesture {
    Drag {
        to: RecordRect,
        duration_ms: u64,
    },
    LongPress {
        duration_ms: u64,
    },
    #[serde(rename = "single_touch_drag_with_vertical_brake_v1")]
    SingleTouchDragWithVerticalBrakeV1 {
        corner: RecordRect,
    },
    #[serde(rename = "single_touch_path_drag_v1")]
    SingleTouchPathDragV1 {
        turns: u64,
        min_segment_px: u64,
        duration_ms: u64,
        corner_hold_ms: u64,
    },
}

impl ClickGesture {
    pub fn duration_ms(self) -> u64 {
        match self {
            Self::Drag { duration_ms, .. } | Self::LongPress { duration_ms } => duration_ms,
            // #411 Lab BD: the horizontal move, the hold at the corner and the brake.
            Self::SingleTouchDragWithVerticalBrakeV1 { .. } => {
                SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS
                    + SEGMENTED_SWIPE_CORNER_HOLD_MS
                    + SEGMENTED_SWIPE_BRAKE_DURATION_MS
            }
            // Workflow #406 T4: its segments and a hold at each of its `turns` inner corners.
            Self::SingleTouchPathDragV1 {
                turns,
                duration_ms,
                corner_hold_ms,
                ..
            } => duration_ms.saturating_add(turns.saturating_mul(corner_hold_ms)),
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::Drag { .. } => "drag",
            Self::LongPress { .. } => "long_press",
            Self::SingleTouchDragWithVerticalBrakeV1 { .. } => {
                "single_touch_drag_with_vertical_brake_v1"
            }
            Self::SingleTouchPathDragV1 { .. } => "single_touch_path_drag_v1",
        }
    }
}

impl StepClick {
    /// A Performed execution is on record.
    pub fn executed(&self) -> bool {
        self.execution
            .as_ref()
            .is_some_and(|execution| execution.effect == "performed")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepTransition {
    Page {
        frames: Vec<RecordedFrame>,
        marks: Vec<RecordedMark>,
        reused: Vec<String>,
        timeout_ms: Option<u64>,
        source: String,
        converted_step: Option<u32>,
    },
    Window {
        min_ms: u64,
        max_ms: u64,
    },
}

impl StepTransition {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Page { .. } => "page",
            Self::Window { .. } => "window",
        }
    }
}

/// `record mark` input; the flag form is converted into this request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkRequest {
    pub schema_version: String,
    #[serde(default)]
    pub step: Option<u32>,
    #[serde(default)]
    pub frame: Option<String>,
    #[serde(default)]
    pub samples: Vec<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub add: Vec<MarkSpec>,
    #[serde(default)]
    pub reuse: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub click: Option<ClickSpec>,
    #[serde(default)]
    pub click_guard: Option<String>,
    #[serde(default)]
    pub retry: Option<ClickRetry>,
    /// Replaces the step's declared effect (a click or an application operation).
    #[serde(default)]
    pub replace_click: bool,
    #[serde(default)]
    pub transition: Option<TransitionSpec>,
    #[serde(default)]
    pub replace_transition: bool,
    #[serde(default)]
    pub step_action: Option<StepAction>,
    /// `{"action": "launch|restart|stop|force-stop"}`: the step's effect is that application
    /// operation (R24).
    #[serde(default)]
    pub application: Option<ApplicationSpec>,
    /// `true` marks the step optional, `false` clears it (Workflow #339).
    #[serde(default)]
    pub optional: Option<bool>,
    /// The settle of `optional: true` (0..=60000); only with it.
    #[serde(default)]
    pub optional_settle_ms: Option<u64>,
    /// Workflow #390 LAB-2: marks of other steps that must not pass on the step's page; `remove`
    /// takes one off again.
    #[serde(default)]
    pub forbid: Vec<String>,
    /// Workflow #390 LAB-2: the own template mark that anchors the step's page.
    #[serde(default)]
    pub anchor: Option<String>,
    /// Workflow #390 LAB-2: the step's page has no anchor any more.
    #[serde(default)]
    pub clear_anchor: bool,
    /// #463 R1 (request form only): the step chooses in a list of fixed slots by the name read in
    /// each slot; its click is the slot the decision selects for `pressed` on the primary frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<SelectSpec>,
    /// #463 R1 fix 1 (request form only): the step's select is taken off; an executed click the
    /// select took over gets back its own rectangle, source and mark (`select.click_before`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_select: bool,
    /// #463 R2: an ending the stopped recording takes after its last press (`record mark
    /// --ending`); alone in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending: Option<EndingSpec>,
    /// #463 R2: the outcome key of the ending the stopped recording gives up (`record mark
    /// --remove-ending <k>`); alone in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_ending: Option<String>,
}

/// #463 R1 (`contracts/lab-recording.md`, "Selecting in a list"): a select over fixed slots, the
/// step's effect in place of a click. `record stop` generates its layout (`fixed_slots`, the one
/// identity feature `name` over `names`), an OCR target per slot, the select operation with its
/// `on_empty` end and the policy document whose one gate `wanted.<name>` keeps `default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectSpec {
    /// 1 to [`LAB_RECORDING_MAX_SELECT_SLOTS`] slots, slot `k` the candidate with instance index
    /// `k`.
    pub slots: Vec<SelectSlotSpec>,
    /// The OCR members every slot's name is read with.
    pub read: SelectReadSpec,
    /// The identity domain: every name the list may show.
    pub names: Vec<SelectNameSpec>,
    /// How a read text is mapped to a name (`ocr_aliases`).
    #[serde(rename = "match")]
    pub matching: SelectMatchSpec,
    /// The wanted value: its name (the parameter grammar) and its value, one of `names`.
    pub wanted: SelectWantedSpec,
    /// The name the agent presses now, as a demonstration; absent means `wanted.default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressed: Option<String>,
    /// The key of the run's normal end when the wanted name is on no slot; absent means
    /// `<task_id>_not_found`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_empty_key: Option<String>,
    /// How the choice is confirmed on a fresh frame; absent is the kernel's default
    /// (`candidate_set`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<SelectConfirmSpec>,
}

impl SelectSpec {
    /// The name the demonstration presses: `pressed`, else the wanted value's default.
    pub fn pressed_name(&self) -> &str {
        self.pressed.as_deref().unwrap_or(&self.wanted.default)
    }
}

/// #463 R1: the most slots one select declares, the projection's OCR budget of one frame.
pub const LAB_RECORDING_MAX_SELECT_SLOTS: usize = 16;

/// #463 R1: one slot: its rectangle, the box a tap is drawn in (absent: the rectangle) and the
/// region its name is read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectSlotSpec {
    pub rect: RecordRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<RecordRect>,
    pub name: RecordRect,
}

impl SelectSlotSpec {
    /// The box a tap on the slot is drawn in.
    pub fn click_rect(&self) -> RecordRect {
        self.click.unwrap_or(self.rect)
    }
}

/// #463 R1: the OCR members of the slots' name reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectReadSpec {
    pub languages: Vec<String>,
    pub timeout_ms: u64,
    pub minimum_confidence_milli: u16,
    pub model_ref: String,
    pub model_sha256: String,
}

/// #463 R1: one name of the identity domain: its id and the texts it is read as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectNameSpec {
    pub id: String,
    pub aliases: Vec<String>,
}

/// #463 R1: the `ocr_aliases` mapping's distance and margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectMatchSpec {
    pub max_distance: u16,
    pub minimum_margin: u16,
}

/// #463 R1: the wanted value: its name and its default, a constant of the package until a later
/// slice reads it as a variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectWantedSpec {
    pub name: String,
    pub default: String,
}

/// #463 R1: how a single-frame select over fixed slots confirms its choice (`track` needs a
/// `repeated_anchor` layout and is not offered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectConfirmSpec {
    CandidateSet,
    Decision,
}

/// #463 R1: a step's select as `recording.json` keeps it: the declaration as given, when it was
/// marked, and its last self-test. Fix 1: when the select took over a click with an outcome on
/// record (a tap recorded before the select, also after `record stop`), that click's own
/// rectangle, source and mark (`click_before`), which `clear_select` gives back; absent otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSelect {
    pub declaration: SelectSpec,
    pub marked_at_unix_ms: u64,
    pub self_test: SelectSelfTest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_before: Option<SelectClickBefore>,
}

/// #463 R1 fix 1: the rectangle, source and mark of an executed click before a select took it
/// over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectClickBefore {
    pub rect: RecordRect,
    pub source: String,
    pub from: Option<String>,
}

/// #463 R1: the mark-time self-test of a select, by the kernel's dry run of the step as `record
/// stop` generates it: `passed` with each live frame's decisions, or `not_evaluated`
/// (`lab_ocr_unavailable`) without an OCR model, never passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectSelfTest {
    pub status: SelfTestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub frames: Vec<SelectFrameDecision>,
}

/// #463 R1: the decisions of one live frame: the kernel's decision for the pressed name and for
/// the wanted value's default, each by its kind (fix 1: a decision that is not `selected` is
/// `empty` only when the run would end normally with the `on_empty` key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectFrameDecision {
    pub frame_id: String,
    pub pressed: SelectDecision,
    pub default: SelectDecision,
}

/// #463 R1 fix 1: one decision of the kernel's dry run of a select on a frame, by its kind:
/// `selected` with its slot; `empty`, the run's normal end with the `on_empty` key; `ambiguous`,
/// `insufficient` and `unknown`, the evaluator's kinds of a decision the run fails on; `blocked`,
/// an empty or insufficient decision while a slot reads no known name, which the run refuses
/// since that slot may hold the wanted name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SelectDecision {
    Selected { slot: u32 },
    Empty,
    Ambiguous,
    Insufficient,
    Blocked,
    Unknown,
}

impl SelectDecision {
    /// The slot a `selected` decision chose; `None` for every other kind.
    pub fn slot(self) -> Option<u32> {
        match self {
            Self::Selected { slot } => Some(slot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSpec {
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkSpec {
    pub id: String,
    pub family: MarkFamily,
    #[serde(default)]
    pub region: Option<RecordRect>,
    #[serde(default)]
    pub search: Option<RecordRect>,
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub max_distance: Option<f64>,
    #[serde(default)]
    pub columns: Option<u32>,
    #[serde(default)]
    pub rows: Option<u32>,
    #[serde(default)]
    pub max_mean_milli: Option<u32>,
    #[serde(default)]
    pub max_cell: Option<u32>,
    #[serde(default)]
    pub exclude_cells: Option<Vec<u32>>,
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub match_mode: Option<String>,
    #[serde(default)]
    pub expected: Option<Vec<String>>,
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    #[serde(default)]
    pub minimum_confidence: Option<f64>,
    #[serde(default)]
    pub model_ref: Option<String>,
    #[serde(default)]
    pub model_sha256: Option<String>,
    /// #463 L2: an OCR mark's `locate`, which makes it a located-text mark.
    #[serde(default)]
    pub locate: Option<OcrLocateSpec>,
    #[serde(default)]
    pub all_of: Option<Vec<String>>,
    #[serde(default)]
    pub any_of: Option<Vec<String>>,
    /// Workflow #396 REC-4: a template mark's factors (REC-5), mask (REC-7) and margin of
    /// uniqueness (REC-2); #463 L2: a located-text mark's margin of uniqueness.
    #[serde(default)]
    pub scales: Option<TemplateScalesSpec>,
    #[serde(default)]
    pub mask: Option<TemplateMaskSpec>,
    #[serde(default)]
    pub unique: Option<TemplateUniqueSpec>,
    /// #411 Lab B-9: a value-test mark's left side, comparison and right side.
    #[serde(default)]
    pub left: Option<ValueTestSide>,
    #[serde(default)]
    pub op: Option<ValueTestOp>,
    #[serde(default)]
    pub right: Option<ValueTestRight>,
}

impl MarkSpec {
    /// A region-only mark of `family` (the flag form `--template`/`--color`).
    pub fn region_only(id: String, family: MarkFamily, region: RecordRect) -> Self {
        Self {
            id,
            family,
            region: Some(region),
            search: None,
            threshold: None,
            max_distance: None,
            columns: None,
            rows: None,
            max_mean_milli: None,
            max_cell: None,
            exclude_cells: None,
            languages: None,
            timeout_ms: None,
            match_mode: None,
            expected: None,
            case_sensitive: None,
            minimum_confidence: None,
            model_ref: None,
            model_sha256: None,
            locate: None,
            all_of: None,
            any_of: None,
            scales: None,
            mask: None,
            unique: None,
            left: None,
            op: None,
            right: None,
        }
    }
}

/// A click declaration: a rectangle (`region`), a mark's region (`from`), or (Workflow #396
/// REC-4) a template mark it follows (`follow`), alone or with a rectangle. Workflow #390 LAB-2:
/// a drag from `region` to `drag_to`, or (Workflow #390 lpbox) a long press in the click box
/// `long_press`, each over `duration_ms`. #411 Lab BD: a single-touch drag with a vertical brake
/// from `region` to the corner box `brake_corner`, with the Runtime's fixed timings. Workflow #406
/// T4: a path drag in the box `region` (`path_drag`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickSpec {
    #[serde(default)]
    pub region: Option<RecordRect>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub follow: Option<String>,
    #[serde(default)]
    pub drag_to: Option<RecordRect>,
    #[serde(default)]
    pub long_press: Option<RecordRect>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub brake_corner: Option<RecordRect>,
    #[serde(default)]
    pub path_drag: Option<PathDragSpec>,
}

/// Workflow #406 T4 (plan Q17; `contracts/resource-declarations.md`, "The path drag"): the numbers
/// of a path drag in the click's box: how often it turns (1..=30), its shortest segment in pixels
/// (1..=(max(width, height) - 1) / 2), its whole duration (`turns + 1 ..= (turns + 1) * 60000`
/// ms) and the hold at each inner corner (0..=60000 ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathDragSpec {
    pub turns: u64,
    pub min_segment_px: u64,
    pub duration_ms: u64,
    pub corner_hold_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransitionSpec {
    #[serde(rename = "none")]
    Clear,
    Page {
        #[serde(default)]
        frame: Option<String>,
        #[serde(default)]
        samples: Vec<String>,
        #[serde(default)]
        add: Vec<MarkSpec>,
        #[serde(default)]
        reuse: Vec<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Window {
        min_ms: u64,
        max_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepActionKind {
    DropStep,
    ReopenStep,
    CloseStep,
    ToTransition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepAction {
    pub kind: StepActionKind,
    #[serde(default)]
    pub step: Option<u32>,
}

/// `record start` options; game, server and locale are already canonical.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordStartOptions {
    pub game: Option<String>,
    pub server: Option<String>,
    pub locale: Option<String>,
    pub match_metric: Option<String>,
    pub template_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabRecordingStart {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub path: String,
    pub record_id: String,
    pub defaults: RecordingDefaults,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepStateView {
    pub marks: usize,
    pub frames: usize,
    /// `none`, `click_declared`, `click_executed`, `click_indeterminate`,
    /// `application_declared` or `application_executed`.
    pub effect: String,
    pub transition: Option<String>,
    pub closed: bool,
    pub optional: Option<StepOptionalView>,
    /// Workflow #390 LAB-2: the marks the page forbids and its anchor, when it has them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

/// `optional` of `step_state` and `record status` steps: `null` or `{settle_ms}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepOptionalView {
    pub settle_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransitionView {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<Vec<FrameView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marks: Option<Vec<RecordedMark>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkOutcome {
    pub status: String,
    pub record_id: String,
    pub step: Option<u32>,
    pub step_opened: bool,
    pub frame: Option<RecordedFrame>,
    pub samples: Vec<RecordedFrame>,
    pub marks: Vec<RecordedMark>,
    pub reused: Vec<RecordedMark>,
    pub removed: Vec<String>,
    pub click: Option<StepClick>,
    pub application: Option<StepApplication>,
    pub transition: Option<TransitionView>,
    pub step_state: Option<StepStateView>,
    pub closed_step: Option<u32>,
    pub dry_run: bool,
    /// Workflow #390 LAB-2: every mark the step's page forbids, tested on its live frames.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<ForbiddenMarkView>,
    /// Workflow #390 LAB-2: the recording is stopped; the next `record stop` generates its
    /// package again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub after_stop: bool,
    /// #463 R1: the step's select with its self-test, on a step that has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<StepSelect>,
    /// #463 R2: the ending the command took, with its marks' self-tests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ending: Option<RecordingEnding>,
    /// #463 R2: the ending the command gave up, as the recording held it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_ending: Option<RecordingEnding>,
}

/// Workflow #390 LAB-2: a mark the step's page forbids, tested on every live frame of the step:
/// `absent` when it passes on none, `not_evaluated` when it could not be evaluated there (an OCR
/// mark without an OCR model).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ForbiddenMarkView {
    pub id: String,
    pub family: MarkFamily,
    pub status: String,
    pub frames: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameView {
    pub frame_id: String,
    pub role: String,
    pub sha256: String,
    pub w: u32,
    pub h: u32,
    pub superseded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkStatusView {
    pub status: SelfTestStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkView {
    pub id: String,
    pub family: MarkFamily,
    pub self_test: MarkStatusView,
    pub margin: Option<SelfTestMargin>,
    /// #411 Lab B-9: a value-test mark's declaration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_test: Option<ValueTestSpec>,
    /// #463 L2 fix 2: a located-text mark as the recording holds it, every member that a request
    /// changing only its names gives again (`region`, `unique`, the OCR fields with `locate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub located_text: Option<RecordedMark>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClickView {
    pub rect: RecordRect,
    pub source: String,
    pub executed: bool,
    pub outcome: String,
    pub attempts: usize,
    pub needs_review: bool,
    /// Workflow #390 LAB-2: a drag or a long press.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<ClickGesture>,
    /// #463 L2 fix 1: the names of the located-text mark the click follows were changed after its
    /// press, so the recorded press is not what the package presses.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub names_changed_after_press: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepView {
    pub index: u32,
    pub artifact_step: Option<u32>,
    /// `any` for the application entry step (no frame), `page` otherwise.
    pub entry: String,
    pub page: Option<String>,
    pub dropped: bool,
    pub converted_to_transition: bool,
    pub frames: Vec<FrameView>,
    pub marks: Vec<MarkView>,
    pub reused: Vec<String>,
    pub click: Option<ClickView>,
    pub application: Option<StepApplication>,
    pub optional: Option<StepOptionalView>,
    pub transition: Option<TransitionView>,
    pub closed: bool,
    pub closed_by: Option<String>,
    /// Workflow #390 LAB-2: the marks the page forbids and its anchor, when it has them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// #463 R1: the step's select as the recording holds it, on a step that has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<StepSelect>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabStatusView {
    pub record_id: String,
    pub status: String,
    pub coordinate_space: Option<RecordSize>,
    pub defaults: RecordingDefaults,
    pub open_step: Option<u32>,
    pub steps: Vec<StepView>,
    pub artifact: Option<RecordingArtifact>,
    /// Workflow #390 LAB-2: a stopped recording marked again since its package was generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amended_at_unix_ms: Option<u64>,
    /// #463 R2: the recording's endings as it holds them, when it has any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub endings: Vec<RecordingEnding>,
}

/// `record status` lab field: unavailable with a reason, or the recording view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LabStatus {
    Unavailable { status: String, reason: String },
    Recording(Box<LabStatusView>),
}

/// Frame bytes from a `--record` command (`capture`, `observe --capture`).
#[derive(Debug, Clone, PartialEq)]
pub struct AttachFrameRequest {
    pub png: Vec<u8>,
    pub source: String,
    pub runtime_artifact: Option<OpaqueJson>,
    pub capture_backend: Option<String>,
    pub freshness: Option<OpaqueJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttachFrameOutcome {
    pub status: String,
    pub record_id: String,
    /// Workflow #449 REC-ARCH: when the recording the frame entered started (its session header's
    /// `started_at_unix_ms`); with `record_id` and `step` the answer names where the action went.
    pub record_started_at_unix_ms: u64,
    pub step: u32,
    pub step_opened: bool,
    pub frame: RecordedFrame,
    pub closed_step: Option<u32>,
}

/// `do --capture --record` point choice.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanClickRequest {
    pub tap: Option<RecordPoint>,
    pub tap_rect: Option<RecordRect>,
    /// Workflow #390 LAB-2: `--swipe x1,y1,x2,y2,ms` on a step that declares a drag.
    pub swipe: Option<PlannedSwipe>,
    /// Workflow #406 T4: how the planned point is drawn in the step's click rectangle when the
    /// Lab is given a seed (`--seed`); `None` plans the rectangle's centre.
    pub draw: Option<DriveDraw>,
}

/// Workflow #390 LAB-2: the points and duration `do --capture --record --swipe` gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedSwipe {
    pub from: RecordPoint,
    pub to: RecordPoint,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClickPlan {
    pub record_id: String,
    pub step: u32,
    pub rect: RecordRect,
    pub point: RecordPoint,
    pub point_rule: String,
    pub source: String,
    pub declares_click: bool,
    /// Workflow #390 LAB-2: the step's drag or long press, and the point it ends at (a long press
    /// ends where it began). The Runtime executes a drag as one swipe from `point` to `to_point`
    /// over its duration and (#411 Lab L3) a long press as one long tap at `point` over its
    /// duration, as the kernel presses the `drag` and `long_press` clicks the recording generates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<ClickGesture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_point: Option<RecordPoint>,
    /// Workflow #406 T4: how the planned points were drawn, when the Lab was given a seed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<InputSamplingEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickEffect {
    Performed,
    Indeterminate,
}

/// The Runtime result of the planned click, as the CLI projected it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitClickRequest {
    pub effect: ClickEffect,
    pub has_failure: bool,
    pub carrier_package: Option<OpaqueJson>,
    pub req_id: Option<OpaqueJson>,
    pub correlation_id: Option<OpaqueJson>,
    pub action_id: Option<OpaqueJson>,
    pub lease_id: Option<OpaqueJson>,
    pub failure: Option<OpaqueJson>,
    pub before: Option<OpaqueJson>,
    pub after: Option<OpaqueJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitClickOutcome {
    pub status: String,
    pub record_id: String,
    /// Workflow #449 REC-ARCH: when the recording the click entered started.
    pub record_started_at_unix_ms: u64,
    pub step: u32,
    pub step_closed: bool,
    pub click: StepClick,
}

/// #463 L1 (`contracts/lab-recording.md`, "One call per step"): `do --capture --record` that
/// records a whole step: the `record mark` request of the open step, the press, and the next
/// frame after it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordStepRequest {
    /// The `actingcommand.lab-record-mark.v1` request `record mark --request-json` takes, applied
    /// first, as `record mark` applies it; `None` is today's `do --capture --record`.
    pub mark: Option<MarkRequest>,
    /// How the press is planned on the open step, as `do --capture --record` plans it.
    pub click: PlanClickRequest,
    /// After a performed press without a failure, wait this many milliseconds (0 to
    /// [`LAB_RECORDING_MAX_NEXT_FRAME_MS`]) and take the next frame as `capture --record` takes
    /// it; `None` takes none.
    pub next_frame_after_ms: Option<u64>,
    /// The file the next frame's PNG is written to, as `capture --record --out` writes it.
    pub next_frame_out: Option<PathBuf>,
}

/// #463 L1: the next frame a one-call step took after its press.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NextFrameOutcome {
    /// The wait before it, in milliseconds.
    pub after_ms: u64,
    /// The file its PNG was written to.
    pub out: Option<String>,
    /// The frame as the recording took it, `capture --record`'s `record`.
    pub record: AttachFrameOutcome,
}

/// #463 L1: what a one-call step recorded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordStepOutcome {
    /// The `record mark` output of the request, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkOutcome>,
    /// The press as `do --capture --record` records it.
    pub click: CommitClickOutcome,
    /// The next frame, when one was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_frame: Option<NextFrameOutcome>,
}

/// #463 S1-1: the wait before the frame after a press (`--after-ms`) when none is given; #463 S1-2:
/// 100, the first of the captures that repeat until the screen is still.
pub const LAB_RECORDING_PRESS_AFTER_MS: u64 = 100;
/// #463 S1-1: the bound of the stillness test after a press (`--still-max-ms`, slice S1-2) when
/// none is given.
pub const LAB_RECORDING_PRESS_STILL_MAX_MS: u64 = 5_000;
/// #463 S1-1: the wait before the frame after an application operation (slice S1-3) when none is
/// given.
pub const LAB_RECORDING_APPLICATION_AFTER_MS: u64 = 2_000;
/// #463 S1-1: the bound of the stillness test after an application operation (slice S1-3) when
/// none is given.
pub const LAB_RECORDING_APPLICATION_STILL_MAX_MS: u64 = 30_000;
/// #463 S1-2 fix round 3: after an application operation, the time from the start of one capture
/// to the start of the next, so that a loading screen of half a minute takes at most 31 captures.
pub const LAB_RECORDING_APPLICATION_INTERVAL_MS: u64 = 1_000;
/// #463 S1-2 fix round 3: after an application operation, how long the screen must not change to
/// be still.
pub const LAB_RECORDING_APPLICATION_STILL_MS: u64 = 2_000;

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): a tap or a long press that
/// `tap --record` or `long-tap --record` declares by its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointPress {
    Tap {
        point: RecordPoint,
    },
    LongPress {
        point: RecordPoint,
        duration_ms: u64,
    },
}

/// #463 S1-1: how the frame after an effect is taken: the wait before it, in milliseconds (0 to
/// [`LAB_RECORDING_MAX_NEXT_FRAME_MS`]), and the bound of the stillness test after it. #463 S1-2
/// runs the test: `interval_ms`, the time from the start of one capture to the start of the next,
/// and `still_ms`, how long the screen must not change to be still (0: no test), each `None` for
/// the recording API's default (`record_after_frame`: 100 and 300).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterCapture {
    pub after_ms: u64,
    pub interval_ms: Option<u64>,
    pub still_ms: Option<u64>,
    pub still_max_ms: u64,
}

impl AfterCapture {
    /// After a press: `after_ms`, else [`LAB_RECORDING_PRESS_AFTER_MS`]; the bound
    /// [`LAB_RECORDING_PRESS_STILL_MAX_MS`]; #463 S1-2: the recording API's interval and stillness
    /// (100, 100, 300, 5000).
    pub fn for_press(after_ms: Option<u64>) -> Self {
        Self {
            after_ms: after_ms.unwrap_or(LAB_RECORDING_PRESS_AFTER_MS),
            interval_ms: None,
            still_ms: None,
            still_max_ms: LAB_RECORDING_PRESS_STILL_MAX_MS,
        }
    }

    /// After an application operation (slice S1-3): `after_ms`, else
    /// [`LAB_RECORDING_APPLICATION_AFTER_MS`]; the bound
    /// [`LAB_RECORDING_APPLICATION_STILL_MAX_MS`]. #463 S1-2 fix round 3: one capture every
    /// [`LAB_RECORDING_APPLICATION_INTERVAL_MS`], still after
    /// [`LAB_RECORDING_APPLICATION_STILL_MS`] unchanged (2000, 1000, 2000, 30000: at most 31
    /// captures, inside the bound of 120 intervals).
    pub fn for_application(after_ms: Option<u64>) -> Self {
        Self {
            after_ms: after_ms.unwrap_or(LAB_RECORDING_APPLICATION_AFTER_MS),
            interval_ms: Some(LAB_RECORDING_APPLICATION_INTERVAL_MS),
            still_ms: Some(LAB_RECORDING_APPLICATION_STILL_MS),
            still_max_ms: LAB_RECORDING_APPLICATION_STILL_MAX_MS,
        }
    }
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): a press `tap`, `swipe` or
/// `long-tap --record` sends and records: a tap or a long press by its point (`point_press`), or a
/// swipe as `do --capture --record --swipe` plans it (`click.swipe`); and how the frame after it is
/// taken.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordPressRequest {
    pub click: PlanClickRequest,
    pub point_press: Option<PointPress>,
    pub after: AfterCapture,
}

/// #463 S1-1: the frame on the screen when the press was committed, the open step's live primary
/// frame; `refreshed` when the press's command took it itself. Fix round 2: every such press is
/// sent on a frame its command captured immediately before it; `sent_on_unstored_capture` when
/// that capture was not stored (the open step carries marks, made on its stored frame), so that
/// `frame_id` names the stored frame and not the one the press was sent on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BeforeFrame {
    pub frame_id: String,
    pub path: String,
    pub sha256: String,
    pub refreshed: bool,
    pub sent_on_unstored_capture: bool,
}

/// #463 S1-1 fix round 2 (`contracts/lab-recording.md`, "Recording while playing"): one frame
/// taken through the Runtime for a press recorded by the command that makes it: the frame as
/// `capture --record` hands it to the recording, and the reference of the frame the Runtime
/// committed for that observation, which an input planned on it names (`do --capture --record`
/// names its before frame so). An observation without a frame id has none (`None`) and no press
/// is sent on it.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedFrame {
    pub frame: AttachFrameRequest,
    pub input_frame: Option<InputFrameReference>,
}

/// #463 S1-1 fix round 2: the frame a press recorded by the command that makes it is sent on, the
/// capture taken immediately before it: the Runtime's reference of it (`input_frame`, handed to
/// the press) and its arrival in the recording (`arrived`), `None` when it was not stored (the
/// open step carries marks).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeCapture {
    pub input_frame: InputFrameReference,
    pub arrived: Option<AttachFrameOutcome>,
}

/// #463 S1-1: the frame after the press and how it was taken. #463 S1-2: the last capture of the
/// press's after loop, the live primary frame of the step the loop's first capture opened (`step`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AfterFrame {
    pub step: u32,
    pub frame_id: String,
    pub path: String,
    pub sha256: String,
    #[serde(flatten)]
    pub record: AfterCaptureRecord,
}

/// #463 S1-1: what a press recorded by the command that makes it recorded, as the command's answer
/// carries it in `record`: the press (`status` `press_recorded`, the recording, the step and its
/// click), the frame before it and the frame after it, and the warnings (none in slice S1-1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordPressOutcome {
    #[serde(flatten)]
    pub click: CommitClickOutcome,
    pub before: BeforeFrame,
    pub after: Option<AfterFrame>,
    pub warnings: Vec<String>,
}

/// `session app <verb> --record`: the verb as given (`launch`, `restart`, `stop`,
/// `force-stop`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanApplicationRequest {
    pub verb: String,
}

/// The step an application operation lands on, planned before anything is sent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApplicationPlan {
    pub record_id: String,
    pub step: u32,
    /// The operation opens the application entry step (the recording has no effective step).
    pub opens_entry_step: bool,
    pub action: String,
    pub cli_verb: String,
}

/// The Runtime result of the planned application operation: a completed receipt, or none
/// that proves it did not run (it may have run).
#[derive(Debug, Clone, PartialEq)]
pub enum CommitApplicationRequest {
    Performed(ApplicationExecution),
    Indeterminate(ApplicationAttempt),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommitApplicationOutcome {
    pub status: String,
    pub record_id: String,
    /// Workflow #449 REC-ARCH: when the recording the operation entered started.
    pub record_started_at_unix_ms: u64,
    pub step: u32,
    pub step_opened: bool,
    pub step_closed: bool,
    pub application: StepApplication,
}

/// Workflow #449 REC-ARCH: the schema of the manifest `record archive` writes into the recording's
/// own directory (`archive-manifest-<n>.json`).
pub const LAB_RECORDING_ARCHIVE_SCHEMA: &str = "actingcommand.lab-recording-archive.v1";

/// Workflow #449 REC-ARCH: `record archive` options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordArchiveOptions {
    /// The recording named by its id, the id its session header carries (`--record-id`); `None`
    /// names the instance's active recording.
    pub record_id: Option<String>,
    /// The directory the frames, the step table and the manifest are copied to (`--copy-to`): an
    /// absolute path (fix 1: an empty, blank or relative one is refused `validation_failed`) of a
    /// directory that does not exist or is empty.
    pub copy_to: Option<String>,
    /// The build that writes the manifest (the commit of the program that runs the command).
    pub written_by: String,
}

/// Workflow #449 REC-ARCH: a manifest of a recording, its file in the recording's directory.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArchiveManifestRef {
    /// Its file name in the recording's directory.
    pub file: String,
    pub path: String,
    pub sha256: String,
    /// The number of steps (rows) it lists.
    pub steps: usize,
    /// Whether this command wrote it (`false`: an archived recording named again).
    pub written: bool,
}

/// Workflow #449 REC-ARCH: the checked copy of an archived recording.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArchiveCopyOutcome {
    /// The directory copied to.
    pub path: String,
    /// The files copied, relative to it, each checked against the manifest.
    pub files: Vec<String>,
}

/// Workflow #449 REC-ARCH, fix 1: the instance's session header when it does not name the recording
/// `record archive` acted on (one `record start --force` left behind, or an archived one whose
/// header a later `record start` replaced): it belongs to another recording, and the command
/// leaves it as it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArchiveSessionHeader {
    /// The recording the header names; `None` without a header.
    pub record_id: Option<String>,
    /// Its status; `None` without a header.
    pub status: Option<String>,
    /// Always `false`: the command did not write it.
    pub changed: bool,
}

/// Workflow #449 REC-ARCH: what `record archive` did. `ends_session` says that the instance's
/// active recording was archived now: the `record` command then marks its session header
/// `archived`, before any copy.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordArchiveOutcome {
    /// `archived`, or `already_archived` for an archived recording named again.
    pub status: String,
    pub record_id: String,
    pub record_started_at_unix_ms: u64,
    pub instance: String,
    /// The recording's own directory.
    pub path: String,
    pub manifest: ArchiveManifestRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<ArchiveCopyOutcome>,
    /// Fix 1: present when the session header does not name the recording, which it leaves as it
    /// is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_header: Option<ArchiveSessionHeader>,
    #[serde(skip)]
    pub ends_session: bool,
}

/// Workflow #449 REC-ARCH: `record reopen` options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordReopenOptions {
    /// The archived recording named by its id (`--record-id`); `None` names the one the session
    /// header names.
    pub record_id: Option<String>,
}

/// Workflow #449 REC-ARCH: the archived recording `record reopen` makes active again. The Lab
/// writes nothing: the `record` command writes the session header, `names_it` saying whether the
/// header already names the recording (its status alone changes) or a new header replaces it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordReopenOutcome {
    /// `reopened`.
    pub status: String,
    pub record_id: String,
    pub task_id: String,
    pub instance: String,
    pub record_started_at_unix_ms: u64,
    /// The recording's own directory.
    pub path: String,
    /// The latest manifest of the recording, which stays as a file.
    pub manifest: String,
    pub steps: usize,
    /// The recording the session header named before, when the new header replaces it.
    pub replaced_record_id: Option<String>,
    #[serde(skip)]
    pub names_it: bool,
}
