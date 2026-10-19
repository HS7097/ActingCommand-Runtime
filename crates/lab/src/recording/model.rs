// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab recording state (`actingcommand.lab-recording.v1`), the mark request
//! (`actingcommand.lab-record-mark.v1`) and the outcome views the CLI prints.

use actingcommand_contract::{
    LabError, LabResult, OcrUnsignedIntegerFormat, OcrValueTrim, ValueTestOp,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const LAB_RECORDING_SCHEMA: &str = "actingcommand.lab-recording.v1";
pub const LAB_RECORD_MARK_SCHEMA: &str = "actingcommand.lab-record-mark.v1";
pub const LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD: f64 = 0.95;
pub const LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE: u32 = 20;
pub const LAB_RECORDING_DEFAULT_MATCH_METRIC: &str = "ccoeff_normed";
/// The settle of `record mark --optional` without `--settle-ms` on a step not optional yet.
pub const LAB_RECORDING_DEFAULT_SETTLE_MS: u64 = 2_000;
/// The largest settle of an optional step (the package's largest step timeout).
pub const LAB_RECORDING_MAX_SETTLE_MS: u64 = 60_000;
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
    pub transition: Option<StepTransition>,
    pub closed: bool,
    pub closed_by: Option<String>,
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
/// template mark, as the task's `verify_templates[].unique` declares it.
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

/// The OCR declaration fields of a task 0.9 `ocr_targets[]` entry; all are required.
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
    /// uniqueness (REC-2), as declared.
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
/// matched at, for a template with `scales`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickFollow {
    pub mark_id: String,
    pub form: ClickFollowForm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<RecordRect>,
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
/// (Workflow #390 lpbox), in which the kernel picks the point it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClickGesture {
    Drag { to: RecordRect, duration_ms: u64 },
    LongPress { duration_ms: u64 },
}

impl ClickGesture {
    pub fn duration_ms(self) -> u64 {
        match self {
            Self::Drag { duration_ms, .. } | Self::LongPress { duration_ms } => duration_ms,
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::Drag { .. } => "drag",
            Self::LongPress { .. } => "long_press",
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
    #[serde(default)]
    pub all_of: Option<Vec<String>>,
    #[serde(default)]
    pub any_of: Option<Vec<String>>,
    /// Workflow #396 REC-4: a template mark's factors (REC-5), mask (REC-7) and margin of
    /// uniqueness (REC-2).
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
/// `long_press`, each over `duration_ms`.
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
    pub step: u32,
    pub step_closed: bool,
    pub click: StepClick,
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
    pub step: u32,
    pub step_opened: bool,
    pub step_closed: bool,
    pub application: StepApplication,
}
