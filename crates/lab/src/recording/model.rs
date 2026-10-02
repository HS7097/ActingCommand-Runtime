// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab recording state (`actingcommand.lab-recording.v1`), the mark request
//! (`actingcommand.lab-record-mark.v1`) and the outcome views the CLI prints.

use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const LAB_RECORDING_SCHEMA: &str = "actingcommand.lab-recording.v1";
pub const LAB_RECORD_MARK_SCHEMA: &str = "actingcommand.lab-record-mark.v1";
pub const LAB_RECORDING_DEFAULT_TEMPLATE_THRESHOLD: f64 = 0.95;
pub const LAB_RECORDING_DEFAULT_COLOR_MAX_DISTANCE: u32 = 20;
pub const LAB_RECORDING_DEFAULT_MATCH_METRIC: &str = "ccoeff_normed";

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
}

/// Written by `record stop` (#336 L4); L3 only carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingArtifact {
    pub container: String,
    pub digest: String,
    pub path: String,
    pub lab_dir_path: Option<String>,
    pub sha256: String,
    pub byte_count: u64,
    pub package_id: String,
    pub generated_at_unix_ms: u64,
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
    pub transition: Option<StepTransition>,
    pub closed: bool,
    pub closed_by: Option<String>,
}

impl RecordingStep {
    /// Neither dropped nor converted into the transition of the previous step.
    pub fn is_effective(&self) -> bool {
        !self.dropped && !self.converted_to_transition
    }
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
}

impl MarkFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Color => "color",
            Self::ColorDigest => "color_digest",
            Self::Ocr => "ocr",
            Self::Check => "check",
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepClick {
    pub rect: RecordRect,
    pub source: String,
    pub from: Option<String>,
    pub retry: Option<ClickRetry>,
    pub declared_at_unix_ms: u64,
    pub execution: Option<ClickExecution>,
    pub attempts: Vec<ClickExecution>,
    pub needs_review: bool,
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
    #[serde(default)]
    pub replace_click: bool,
    #[serde(default)]
    pub transition: Option<TransitionSpec>,
    #[serde(default)]
    pub replace_transition: bool,
    #[serde(default)]
    pub step_action: Option<StepAction>,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickSpec {
    #[serde(default)]
    pub region: Option<RecordRect>,
    #[serde(default)]
    pub from: Option<String>,
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
    pub click: String,
    pub transition: Option<String>,
    pub closed: bool,
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
    pub transition: Option<TransitionView>,
    pub step_state: Option<StepStateView>,
    pub closed_step: Option<u32>,
    pub dry_run: bool,
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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClickView {
    pub rect: RecordRect,
    pub source: String,
    pub executed: bool,
    pub outcome: String,
    pub attempts: usize,
    pub needs_review: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepView {
    pub index: u32,
    pub artifact_step: Option<u32>,
    pub page: Option<String>,
    pub dropped: bool,
    pub converted_to_transition: bool,
    pub frames: Vec<FrameView>,
    pub marks: Vec<MarkView>,
    pub reused: Vec<String>,
    pub click: Option<ClickView>,
    pub transition: Option<TransitionView>,
    pub closed: bool,
    pub closed_by: Option<String>,
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
