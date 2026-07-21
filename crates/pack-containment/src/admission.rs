// SPDX-License-Identifier: AGPL-3.0-only

//! Versioned wire parsing and canonical executable-package admission.
//!
//! Raw DTOs in this module are deliberately private. Package consumers only receive the
//! canonical capability defined below; they never receive a serde document to reinterpret.

use super::{
    ContainmentError, ContainmentResult, PackageLayout, PackageMetadata, Sha256Hash,
    package_contract_error, prefixed_path, read_json_entry, validate_relative_ref,
};
use actingcommand_page_detector::{PageDefinition, PageDetector, PageSet};
use actingcommand_recognition_pack::{
    AssetResolver, ClickOnlyTarget, ColorCheck, ColorTarget, PackCoordinateSpace, PackRect,
    PackRegion, RecognitionDefaults, RecognitionEvaluator, RecognitionMask, RecognitionMatchMetric,
    RecognitionMethod, RecognitionPack, RecognitionPackError, RecognitionTarget, TargetKind,
    TemplateTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

const CONTROL_SCHEMA_V1: &str = "Lab-1y.control.v1";
const MANIFEST_SCHEMA_V03: &str = "0.3";
const OPERATION_SCHEMAS: &[&str] = &["0.3", "0.4", "0.5", "0.6"];
const RECOGNITION_SCHEMAS: &[&str] = &["0.1", "0.3", "0.4", "0.5"];
const PAGE_SCHEMAS: &[&str] = &["0.1", "0.3", "0.4", "0.5"];
const NAVIGATION_SCHEMAS: &[&str] = &["0.3", "0.4", "0.5"];

const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 50;
const DEFAULT_TASK_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_STEPS: u32 = 100;
const MAX_CAPTURE_INTERVAL_MS: u64 = 5_000;
const MAX_TASK_TIMEOUT_MS: u64 = 600_000;
const MAX_STEP_TIMEOUT_MS: u64 = 60_000;
const MAX_STEPS: u32 = 1_000;
const MAX_INPUT_DURATION_MS: u64 = 60_000;
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.90;
const DEFAULT_COLOR_MAX_DISTANCE: f32 = 20.0;

macro_rules! string_key {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            fn parse(value: impl Into<String>, code: &'static str) -> AdmissionResult<Self> {
                let value = value.into();
                if value.trim().is_empty() || value != value.trim() {
                    return Err(AdmissionError::new(code));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_key!(TaskKey);
string_key!(TargetKey);
string_key!(AssetKey);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct OperationKey {
    task: TaskKey,
    operation: String,
}

impl OperationKey {
    fn new(task: TaskKey, operation: impl Into<String>) -> AdmissionResult<Self> {
        let operation = operation.into();
        if operation.trim().is_empty() || operation != operation.trim() {
            return Err(AdmissionError::new("admission_identity_invalid"));
        }
        Ok(Self { task, operation })
    }

    pub fn task(&self) -> &TaskKey {
        &self.task
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }
}

impl fmt::Display for OperationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.task, self.operation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct PageKey {
    game: String,
    page: String,
}

impl PageKey {
    fn parse(game: &str, value: &str) -> AdmissionResult<Self> {
        if game.trim().is_empty() || value.trim().is_empty() || value != value.trim() {
            return Err(AdmissionError::new("admission_page_invalid"));
        }
        let prefix = format!("{game}/");
        let page = if let Some(page) = value.strip_prefix(&prefix) {
            page
        } else if value.contains('/') {
            return Err(AdmissionError::with_detail(
                "admission_page_invalid",
                format!("page '{value}' is qualified for a different game"),
            ));
        } else {
            value
        };
        if page.is_empty() || page == "any" {
            return Err(AdmissionError::new("admission_page_invalid"));
        }
        Ok(Self {
            game: game.to_string(),
            page: page.to_string(),
        })
    }

    pub fn game(&self) -> &str {
        &self.game
    }

    pub fn page(&self) -> &str {
        &self.page
    }

    pub fn qualified(&self) -> String {
        format!("{}/{}", self.game, self.page)
    }
}

impl fmt::Display for PageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.game, self.page)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "page", rename_all = "snake_case")]
pub enum PageSelector {
    Any,
    Exact(PageKey),
}

impl PageSelector {
    pub fn matches(&self, page: &PageKey) -> bool {
        matches!(self, Self::Any) || matches!(self, Self::Exact(expected) if expected == page)
    }

    pub fn exact(&self) -> Option<&PageKey> {
        match self {
            Self::Any => None,
            Self::Exact(page) => Some(page),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    RecognizeOnly,
    NavigableRoute,
    InPageGuard,
}

impl ExecutionMode {
    fn parse(value: &str) -> AdmissionResult<Self> {
        match value {
            "recognize_only" => Ok(Self::RecognizeOnly),
            "navigable_route" => Ok(Self::NavigableRoute),
            "in_page_guard" => Ok(Self::InPageGuard),
            _ => Err(AdmissionError::new("admission_mode_invalid")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RecognizeOnly => "recognize_only",
            Self::NavigableRoute => "navigable_route",
            Self::InPageGuard => "in_page_guard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PackageResolution {
    width: u32,
    height: u32,
}

impl PackageResolution {
    fn new(width: u32, height: u32) -> AdmissionResult<Self> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(AdmissionError::new("admission_resolution_invalid"));
        }
        Ok(Self { width, height })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoundedPoint {
    x: i32,
    y: i32,
}

impl BoundedPoint {
    fn new(x: i32, y: i32, resolution: PackageResolution) -> AdmissionResult<Self> {
        if x < 0 || y < 0 || x as u32 >= resolution.width || y as u32 >= resolution.height {
            return Err(AdmissionError::new("admission_input_bounds_invalid"));
        }
        Ok(Self { x, y })
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoundedRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl BoundedRect {
    fn new(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        resolution: PackageResolution,
    ) -> AdmissionResult<Self> {
        if width <= 0 || height <= 0 {
            return Err(AdmissionError::new("admission_input_bounds_invalid"));
        }
        let right = x
            .checked_add(width - 1)
            .ok_or_else(|| AdmissionError::new("admission_input_bounds_invalid"))?;
        let bottom = y
            .checked_add(height - 1)
            .ok_or_else(|| AdmissionError::new("admission_input_bounds_invalid"))?;
        BoundedPoint::new(x, y, resolution)?;
        BoundedPoint::new(right, bottom, resolution)?;
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn from_raw(rect: RawRect, resolution: PackageResolution) -> AdmissionResult<Self> {
        Self::new(rect.x, rect.y, rect.width, rect.height, resolution)
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }

    pub const fn width(self) -> i32 {
        self.width
    }

    pub const fn height(self) -> i32 {
        self.height
    }

    pub fn center(self) -> BoundedPoint {
        // i64 arithmetic keeps this total even at the i32 boundary; construction proves the
        // result fits the package resolution and therefore i32.
        BoundedPoint {
            x: (i64::from(self.x) + i64::from(self.width / 2)) as i32,
            y: (i64::from(self.y) + i64::from(self.height / 2)) as i32,
        }
    }

    pub fn intersects(self, other: Self) -> bool {
        let self_right = i64::from(self.x) + i64::from(self.width);
        let self_bottom = i64::from(self.y) + i64::from(self.height);
        let other_right = i64::from(other.x) + i64::from(other.width);
        let other_bottom = i64::from(other.y) + i64::from(other.height);
        i64::from(self.x) < other_right
            && self_right > i64::from(other.x)
            && i64::from(self.y) < other_bottom
            && self_bottom > i64::from(other.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct InputDuration(u64);

impl InputDuration {
    fn new(value: u64) -> AdmissionResult<Self> {
        if !(1..=MAX_INPUT_DURATION_MS).contains(&value) {
            return Err(AdmissionError::new("admission_input_duration_invalid"));
        }
        Ok(Self(value))
    }

    pub const fn milliseconds(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TargetOffset {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl TargetOffset {
    fn new(rect: RawRect, resolution: PackageResolution) -> AdmissionResult<Self> {
        let rect = BoundedRect::from_raw(rect, resolution)?;
        Ok(Self {
            x: rect.x(),
            y: rect.y(),
            width: rect.width(),
            height: rect.height(),
        })
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }

    pub const fn width(self) -> i32 {
        self.width
    }

    pub const fn height(self) -> i32 {
        self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetTapMode {
    Deterministic,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmittedEffectCapability {
    NavigationOnly,
    Destructive,
}

impl AdmittedEffectCapability {
    pub const fn requires_explicit_opt_in(self) -> bool {
        matches!(self, Self::Destructive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdmittedAction {
    Tap {
        rect: BoundedRect,
        point: BoundedPoint,
    },
    LongTap {
        point: BoundedPoint,
        duration: InputDuration,
    },
    Drag {
        from_rect: BoundedRect,
        to_rect: BoundedRect,
        from: BoundedPoint,
        to: BoundedPoint,
        duration: InputDuration,
    },
    TargetTap {
        target: TargetKey,
        mode: TargetTapMode,
        offset: Option<TargetOffset>,
    },
}

impl AdmittedAction {
    pub fn static_rects(&self) -> Vec<BoundedRect> {
        match self {
            Self::Tap { rect, .. } => vec![*rect],
            Self::LongTap { point, .. } => vec![BoundedRect {
                x: point.x,
                y: point.y,
                width: 1,
                height: 1,
            }],
            Self::Drag {
                from_rect, to_rect, ..
            } => vec![*from_rect, *to_rect],
            Self::TargetTap { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedGuard {
    page: PageKey,
    target: TargetKey,
    expected_rect: BoundedRect,
    verification: GuardVerification,
}

impl AdmittedGuard {
    pub fn page(&self) -> &PageKey {
        &self.page
    }

    pub fn target(&self) -> &TargetKey {
        &self.target
    }

    pub const fn expected_rect(&self) -> BoundedRect {
        self.expected_rect
    }

    pub fn verification(&self) -> &GuardVerification {
        &self.verification
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuardVerification {
    Template { asset: AssetKey },
    Color { probe: TargetKey },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpaqueMetadata(Value);

impl OpaqueMetadata {
    fn new(value: Value) -> Self {
        Self(canonicalize_json_value(value))
    }
}

fn canonicalize_json_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize_json_value).collect())
        }
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json_value(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct FrameStoreSettings {
    similarity_threshold: Option<f32>,
    tier1_ratio: Option<f64>,
    tier2_ratio: Option<f64>,
    tier3_ratio: Option<f64>,
    hysteresis_ratio: Option<f64>,
    max_mem_bytes: Option<u64>,
    os_reserve_bytes: Option<u64>,
    flush_workspace_reserve_bytes: Option<u64>,
}

impl FrameStoreSettings {
    pub const fn similarity_threshold(self) -> Option<f32> {
        self.similarity_threshold
    }
    pub const fn tier1_ratio(self) -> Option<f64> {
        self.tier1_ratio
    }
    pub const fn tier2_ratio(self) -> Option<f64> {
        self.tier2_ratio
    }
    pub const fn tier3_ratio(self) -> Option<f64> {
        self.tier3_ratio
    }
    pub const fn hysteresis_ratio(self) -> Option<f64> {
        self.hysteresis_ratio
    }
    pub const fn max_mem_bytes(self) -> Option<u64> {
        self.max_mem_bytes
    }
    pub const fn os_reserve_bytes(self) -> Option<u64> {
        self.os_reserve_bytes
    }
    pub const fn flush_workspace_reserve_bytes(self) -> Option<u64> {
        self.flush_workspace_reserve_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdmittedControl {
    package_id: String,
    execution_mode: ExecutionMode,
    game: String,
    server: String,
    resolution: PackageResolution,
    entry_task: TaskKey,
    capture_interval_ms: u64,
    timeout_ms: u64,
    step_timeout_ms: u64,
    max_steps: u32,
    stop_on_error: Option<bool>,
    stop_on_confirmation: bool,
    allow_placeholder_coords: bool,
    output: Option<OpaqueMetadata>,
    capture_backend: Option<String>,
    frame_store: FrameStoreSettings,
    producer_present: bool,
    trusted_execution_present: bool,
}

impl AdmittedControl {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    pub const fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }
    pub fn game(&self) -> &str {
        &self.game
    }
    pub fn server(&self) -> &str {
        &self.server
    }
    pub const fn resolution(&self) -> PackageResolution {
        self.resolution
    }
    pub fn entry_task(&self) -> &TaskKey {
        &self.entry_task
    }
    pub const fn capture_interval_ms(&self) -> u64 {
        self.capture_interval_ms
    }
    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
    pub const fn step_timeout_ms(&self) -> u64 {
        self.step_timeout_ms
    }
    pub const fn max_steps(&self) -> u32 {
        self.max_steps
    }
    pub const fn stop_on_error(&self) -> Option<bool> {
        self.stop_on_error
    }
    pub const fn stop_on_confirmation(&self) -> bool {
        self.stop_on_confirmation
    }
    pub const fn allow_placeholder_coords(&self) -> bool {
        self.allow_placeholder_coords
    }
    pub fn output(&self) -> Option<&OpaqueMetadata> {
        self.output.as_ref()
    }
    pub fn capture_backend(&self) -> Option<&str> {
        self.capture_backend.as_deref()
    }
    pub const fn frame_store(&self) -> FrameStoreSettings {
        self.frame_store
    }
    pub const fn producer_present(&self) -> bool {
        self.producer_present
    }
    pub const fn trusted_execution_present(&self) -> bool {
        self.trusted_execution_present
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct AdmittedOperationDefaults {
    template_threshold: f32,
    color_max_distance: Option<f32>,
    timeout_ms: Option<u64>,
    max_attempts: Option<u32>,
    retry_interval_ms: Option<u64>,
    pre_delay_ms: Option<u64>,
    post_delay_ms: Option<u64>,
    pre_wait_freezes_ms: Option<u64>,
    post_wait_freezes_ms: Option<u64>,
}

impl AdmittedOperationDefaults {
    pub const fn template_threshold(self) -> f32 {
        self.template_threshold
    }
    pub const fn color_max_distance(self) -> Option<f32> {
        self.color_max_distance
    }
    pub const fn timeout_ms(self) -> Option<u64> {
        self.timeout_ms
    }
    pub const fn max_attempts(self) -> Option<u32> {
        self.max_attempts
    }
    pub const fn retry_interval_ms(self) -> Option<u64> {
        self.retry_interval_ms
    }
    pub const fn pre_delay_ms(self) -> Option<u64> {
        self.pre_delay_ms
    }
    pub const fn post_delay_ms(self) -> Option<u64> {
        self.post_delay_ms
    }
    pub const fn pre_wait_freezes_ms(self) -> Option<u64> {
        self.pre_wait_freezes_ms
    }
    pub const fn post_wait_freezes_ms(self) -> Option<u64> {
        self.post_wait_freezes_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedAnchor {
    id: String,
    asset: AssetKey,
}

impl AdmittedAnchor {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn asset(&self) -> &AssetKey {
        &self.asset
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedExpectation {
    page: PageKey,
    timeout_ms: Option<u64>,
    interval_ms: Option<u64>,
}

impl AdmittedExpectation {
    pub fn page(&self) -> &PageKey {
        &self.page
    }
    pub const fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }
    pub const fn interval_ms(&self) -> Option<u64> {
        self.interval_ms
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdmittedOperation {
    key: OperationKey,
    purpose: String,
    from: PageSelector,
    to: Option<PageKey>,
    action: AdmittedAction,
    verify_template: Option<AssetKey>,
    expect_after: Option<AdmittedExpectation>,
    timeout_ms: Option<u64>,
    max_attempts: Option<u32>,
    retry_interval_ms: Option<u64>,
    pre_delay_ms: Option<u64>,
    post_delay_ms: Option<u64>,
    pre_wait_freezes_ms: Option<u64>,
    post_wait_freezes_ms: Option<u64>,
    retryable: Option<bool>,
    navigation_only: bool,
    effect_capability: AdmittedEffectCapability,
    on_error: Option<TaskKey>,
    guard: Option<AdmittedGuard>,
    unguarded_trusted_coordinate: bool,
    consumes: Vec<String>,
    produces: Vec<String>,
    verified_live: Option<bool>,
    provenance: Option<OpaqueMetadata>,
}

impl AdmittedOperation {
    pub fn key(&self) -> &OperationKey {
        &self.key
    }
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
    pub fn from(&self) -> &PageSelector {
        &self.from
    }
    pub fn to(&self) -> Option<&PageKey> {
        self.to.as_ref()
    }
    pub fn action(&self) -> &AdmittedAction {
        &self.action
    }
    pub fn verify_template(&self) -> Option<&AssetKey> {
        self.verify_template.as_ref()
    }
    pub fn expect_after(&self) -> Option<&AdmittedExpectation> {
        self.expect_after.as_ref()
    }
    pub const fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }
    pub const fn max_attempts(&self) -> Option<u32> {
        self.max_attempts
    }
    pub const fn retry_interval_ms(&self) -> Option<u64> {
        self.retry_interval_ms
    }
    pub const fn pre_delay_ms(&self) -> Option<u64> {
        self.pre_delay_ms
    }
    pub const fn post_delay_ms(&self) -> Option<u64> {
        self.post_delay_ms
    }
    pub const fn pre_wait_freezes_ms(&self) -> Option<u64> {
        self.pre_wait_freezes_ms
    }
    pub const fn post_wait_freezes_ms(&self) -> Option<u64> {
        self.post_wait_freezes_ms
    }
    pub const fn retryable(&self) -> Option<bool> {
        self.retryable
    }
    pub const fn navigation_only(&self) -> bool {
        self.navigation_only
    }
    pub const fn effect_capability(&self) -> AdmittedEffectCapability {
        self.effect_capability
    }
    pub const fn destructive(&self) -> bool {
        self.effect_capability.requires_explicit_opt_in()
    }
    pub fn on_error(&self) -> Option<&TaskKey> {
        self.on_error.as_ref()
    }
    pub fn guard(&self) -> Option<&AdmittedGuard> {
        self.guard.as_ref()
    }
    pub const fn unguarded_trusted_coordinate(&self) -> bool {
        self.unguarded_trusted_coordinate
    }
    pub fn consumes(&self) -> &[String] {
        &self.consumes
    }
    pub fn produces(&self) -> &[String] {
        &self.produces
    }
    pub const fn verified_live(&self) -> Option<bool> {
        self.verified_live
    }
    pub fn provenance(&self) -> Option<&OpaqueMetadata> {
        self.provenance.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdmittedTask {
    key: TaskKey,
    schema_version: String,
    goal: String,
    defaults: AdmittedOperationDefaults,
    anchors: Vec<AdmittedAnchor>,
    entry_page: Option<PageKey>,
    target_page: Option<PageKey>,
    error_pages: Vec<PageKey>,
    recovery: Option<TaskKey>,
    max_task_retries: Option<u32>,
    pause_on_exhausted: bool,
    operations: Vec<AdmittedOperation>,
}

impl AdmittedTask {
    pub fn key(&self) -> &TaskKey {
        &self.key
    }
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }
    pub fn goal(&self) -> &str {
        &self.goal
    }
    pub const fn defaults(&self) -> AdmittedOperationDefaults {
        self.defaults
    }
    pub fn anchors(&self) -> &[AdmittedAnchor] {
        &self.anchors
    }
    pub fn entry_page(&self) -> Option<&PageKey> {
        self.entry_page.as_ref()
    }
    pub fn target_page(&self) -> Option<&PageKey> {
        self.target_page.as_ref()
    }
    pub fn error_pages(&self) -> &[PageKey] {
        &self.error_pages
    }
    pub fn recovery(&self) -> Option<&TaskKey> {
        self.recovery.as_ref()
    }
    pub const fn max_task_retries(&self) -> Option<u32> {
        self.max_task_retries
    }
    pub const fn pause_on_exhausted(&self) -> bool {
        self.pause_on_exhausted
    }
    pub fn operations(&self) -> &[AdmittedOperation] {
        &self.operations
    }
    pub fn operation(&self, key: &OperationKey) -> Option<&AdmittedOperation> {
        self.operations
            .iter()
            .find(|operation| operation.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedRoute {
    operation: OperationKey,
    source: Option<String>,
}

impl AdmittedRoute {
    pub fn operation(&self) -> &OperationKey {
        &self.operation
    }
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedDestructiveRegion {
    page: PageSelector,
    rect: BoundedRect,
    operation: Option<OperationKey>,
}

impl AdmittedDestructiveRegion {
    pub fn page(&self) -> &PageSelector {
        &self.page
    }
    pub const fn rect(&self) -> BoundedRect {
        self.rect
    }
    pub fn operation(&self) -> Option<&OperationKey> {
        self.operation.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedControlPoint {
    name: String,
    action: AdmittedAction,
    effect_capability: AdmittedEffectCapability,
}

impl AdmittedControlPoint {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn action(&self) -> &AdmittedAction {
        &self.action
    }
    pub const fn effect_capability(&self) -> AdmittedEffectCapability {
        self.effect_capability
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdmittedNavigation {
    schema_version: String,
    routes: Vec<AdmittedRoute>,
    page_operations: Vec<OperationKey>,
    destructive_regions: Vec<AdmittedDestructiveRegion>,
    control_points: Vec<AdmittedControlPoint>,
}

impl AdmittedNavigation {
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }
    pub fn routes(&self) -> &[AdmittedRoute] {
        &self.routes
    }
    pub fn page_operations(&self) -> &[OperationKey] {
        &self.page_operations
    }
    pub fn destructive_regions(&self) -> &[AdmittedDestructiveRegion] {
        &self.destructive_regions
    }
    pub fn control_points(&self) -> &[AdmittedControlPoint] {
        &self.control_points
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmittedTargetKind {
    Template,
    Color,
    ClickOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AdmittedRecognitionMethod {
    Ncc,
    RgbCount,
    HsvCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AdmittedRecognitionMask {
    Range { lower: u8, upper: u8 },
    Bitmap { asset: AssetKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AdmittedColorCheck {
    region: BoundedRect,
    expected: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AdmittedTarget {
    Template {
        asset: AssetKey,
        region: Option<BoundedRect>,
        threshold: Option<f32>,
        method: AdmittedRecognitionMethod,
        mask: Option<AdmittedRecognitionMask>,
        rect_move: Option<BoundedRect>,
        color_check: Option<AdmittedColorCheck>,
        click: Option<BoundedRect>,
    },
    Color {
        region: BoundedRect,
        expected: [u8; 3],
        click: Option<BoundedRect>,
    },
    ClickOnly {
        click: BoundedRect,
    },
}

impl AdmittedTarget {
    const fn kind(&self) -> AdmittedTargetKind {
        match self {
            Self::Template { .. } => AdmittedTargetKind::Template,
            Self::Color { .. } => AdmittedTargetKind::Color,
            Self::ClickOnly { .. } => AdmittedTargetKind::ClickOnly,
        }
    }

    fn template_asset(&self) -> Option<&AssetKey> {
        match self {
            Self::Template { asset, .. } => Some(asset),
            Self::Color { .. } | Self::ClickOnly { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AdmittedPage {
    detector_id: String,
    required: Vec<TargetKey>,
    any_of: Vec<Vec<TargetKey>>,
    optional: Vec<TargetKey>,
    forbidden: Vec<TargetKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AdmittedMatchMetric {
    CcorrNormed,
    CcoeffNormed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct AdmittedRecognitionMetadata {
    schema_version: String,
    page_schema_version: String,
    locale: Option<String>,
    template_threshold: f32,
    color_max_distance: f32,
    match_metric: AdmittedMatchMetric,
}

impl From<TargetKind> for AdmittedTargetKind {
    fn from(value: TargetKind) -> Self {
        match value {
            TargetKind::Template => Self::Template,
            TargetKind::Color => Self::Color,
            TargetKind::ClickOnly => Self::ClickOnly,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdmittedPackage {
    control: AdmittedControl,
    entry_task: AdmittedTask,
    tasks: BTreeMap<TaskKey, AdmittedTask>,
    navigation: Option<AdmittedNavigation>,
    pages: BTreeMap<PageKey, AdmittedPage>,
    targets: BTreeMap<TargetKey, AdmittedTarget>,
    assets: BTreeMap<AssetKey, String>,
    asset_bytes: BTreeMap<AssetKey, Arc<[u8]>>,
    semantic_fingerprint: String,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
}

impl AdmittedPackage {
    pub fn control(&self) -> &AdmittedControl {
        &self.control
    }
    pub fn entry_task(&self) -> &AdmittedTask {
        &self.entry_task
    }
    pub fn task(&self, key: &TaskKey) -> Option<&AdmittedTask> {
        if self.entry_task.key() == key {
            Some(&self.entry_task)
        } else {
            self.tasks.get(key)
        }
    }
    pub fn tasks(&self) -> impl Iterator<Item = &AdmittedTask> {
        std::iter::once(&self.entry_task).chain(self.tasks.values())
    }
    pub fn operation(&self, key: &OperationKey) -> Option<&AdmittedOperation> {
        self.task(key.task()).and_then(|task| task.operation(key))
    }
    pub fn navigation(&self) -> Option<&AdmittedNavigation> {
        self.navigation.as_ref()
    }
    pub fn detector_page_id(&self, key: &PageKey) -> Option<&str> {
        self.pages.get(key).map(|page| page.detector_id.as_str())
    }
    pub fn pages(&self) -> impl Iterator<Item = &PageKey> {
        self.pages.keys()
    }
    pub fn page_key_for_detector_id(&self, detector_id: &str) -> Option<&PageKey> {
        self.pages
            .iter()
            .find_map(|(key, page)| (page.detector_id == detector_id).then_some(key))
    }
    pub fn target_kind(&self, key: &TargetKey) -> Option<AdmittedTargetKind> {
        self.targets.get(key).map(AdmittedTarget::kind)
    }
    pub fn target_operation(&self, target_id: &str) -> Option<&AdmittedOperation> {
        let mut operations = self
            .tasks()
            .flat_map(AdmittedTask::operations)
            .filter(|operation| operation_is_target_tap_authority(operation, target_id));
        let first = operations.next()?;
        operations.next().is_none().then_some(first)
    }
    pub fn assets(&self) -> impl Iterator<Item = (&AssetKey, &str)> {
        self.assets.iter().map(|(key, hash)| (key, hash.as_str()))
    }
    pub fn asset_bytes(&self, key: &AssetKey) -> Option<&[u8]> {
        self.asset_bytes.get(key).map(AsRef::as_ref)
    }
    pub fn semantic_fingerprint(&self) -> &str {
        &self.semantic_fingerprint
    }
    pub fn evaluator(&self) -> &RecognitionEvaluator {
        &self.evaluator
    }
    pub fn detector(&self) -> &PageDetector {
        &self.detector
    }
}

fn operation_is_target_tap_authority(operation: &AdmittedOperation, target_id: &str) -> bool {
    match operation.action() {
        AdmittedAction::TargetTap { target, .. } => target.as_str() == target_id,
        AdmittedAction::Tap { .. } => operation
            .guard()
            .is_some_and(|guard| guard.target().as_str() == target_id),
        AdmittedAction::LongTap { .. } | AdmittedAction::Drag { .. } => false,
    }
}

pub type AdmissionResult<T> = Result<T, AdmissionError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionError {
    code: &'static str,
    detail: Option<String>,
}

impl AdmissionError {
    fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AdmissionError {}

#[derive(Debug)]
pub(super) struct ParsedPackage {
    pub(super) control: RawControlV1,
    pub(super) manifest: RawManifestV03,
    pub(super) tasks: BTreeMap<String, RawTaskDocument>,
    pub(super) recognition: RawRecognitionDocument,
    pub(super) pages: RawPageDocument,
    pub(super) navigation: Option<RawNavigationDocument>,
}

#[derive(Debug)]
pub(super) struct ClosedPackage {
    control: AdmittedControl,
    entry_task: AdmittedTask,
    tasks: BTreeMap<TaskKey, AdmittedTask>,
    navigation: Option<AdmittedNavigation>,
    recognition: AdmittedRecognitionMetadata,
    pages: BTreeMap<PageKey, AdmittedPage>,
    targets: BTreeMap<TargetKey, AdmittedTarget>,
    assets: BTreeMap<AssetKey, String>,
    asset_bytes: BTreeMap<AssetKey, Arc<[u8]>>,
}

pub(super) fn close_package(
    parsed: ParsedPackage,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
) -> AdmissionResult<ClosedPackage> {
    let control = close_control(&parsed.control)?;
    if parsed.manifest.entry_task_id != control.entry_task().as_str() {
        return Err(AdmissionError::with_detail(
            "admission_identity_conflict",
            "manifest entry_task_id does not match control entry_task_id",
        ));
    }
    validate_closed_manifest(&parsed.manifest)?;

    let (recognition, targets, mut assets) = close_recognition(
        &parsed.recognition,
        &parsed.pages.schema_version,
        &control,
        entries,
        resource_root,
    )?;
    let pages = close_pages(&parsed.pages, &control, &targets)?;

    let mut tasks = BTreeMap::new();
    for raw in parsed.tasks.values() {
        let task = close_task(
            raw,
            &control,
            &pages,
            &targets,
            entries,
            resource_root,
            &mut assets,
        )?;
        if tasks.insert(task.key().clone(), task).is_some() {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
    }
    validate_task_graph(control.entry_task(), &tasks)?;

    let navigation = close_navigation(
        parsed.navigation.as_ref(),
        &control,
        &tasks,
        &pages,
        &targets,
    )?;
    if control.execution_mode() == ExecutionMode::NavigableRoute
        && navigation
            .as_ref()
            .is_none_or(|navigation| navigation.routes().is_empty())
    {
        return Err(AdmissionError::new("admission_mode_requirements"));
    }

    let entry_key = control.entry_task().clone();
    let mut remaining_tasks = tasks;
    let entry_task = remaining_tasks.remove(&entry_key).ok_or_else(|| {
        AdmissionError::with_detail(
            "admission_missing_reference",
            format!("entry task '{entry_key}' is not packaged"),
        )
    })?;
    let asset_bytes = assets
        .keys()
        .map(|key| {
            let path = prefixed_path(resource_root, key.as_str());
            let bytes = entries.get(&path).ok_or_else(|| {
                AdmissionError::with_detail(
                    "admission_missing_reference",
                    format!("asset '{}' is not packaged", key.as_str()),
                )
            })?;
            Ok((key.clone(), Arc::<[u8]>::from(bytes.as_slice())))
        })
        .collect::<AdmissionResult<BTreeMap<_, _>>>()?;

    Ok(ClosedPackage {
        control,
        entry_task,
        tasks: remaining_tasks,
        navigation,
        recognition,
        pages,
        targets,
        assets,
        asset_bytes,
    })
}

pub(super) fn admit_package(closed: ClosedPackage) -> AdmissionResult<AdmittedPackage> {
    let semantic_fingerprint = canonical_semantic_fingerprint(&closed)?;
    let (evaluator, detector) = canonical_recognition_pipeline(&closed)?;
    Ok(AdmittedPackage {
        control: closed.control,
        entry_task: closed.entry_task,
        tasks: closed.tasks,
        navigation: closed.navigation,
        pages: closed.pages,
        targets: closed.targets,
        assets: closed.assets,
        asset_bytes: closed.asset_bytes,
        semantic_fingerprint,
        evaluator,
        detector,
    })
}

#[derive(Debug)]
struct CanonicalAssetResolver {
    assets: BTreeMap<String, Arc<[u8]>>,
}

impl AssetResolver for CanonicalAssetResolver {
    fn read_asset(&self, path: &str) -> Result<Vec<u8>, RecognitionPackError> {
        self.assets
            .get(path)
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| {
                RecognitionPackError::fatal(format!("canonical asset '{path}' is missing"))
            })
    }

    fn contains_asset(&self, path: &str) -> bool {
        self.assets.contains_key(path)
    }
}

fn canonical_recognition_pipeline(
    closed: &ClosedPackage,
) -> AdmissionResult<(RecognitionEvaluator, PageDetector)> {
    let recognition = &closed.recognition;
    let pack = RecognitionPack {
        schema_version: recognition.schema_version.clone(),
        game: Some(closed.control.game().to_string()),
        server: Some(closed.control.server().to_string()),
        locale: recognition.locale.clone(),
        coordinate_space: Some(PackCoordinateSpace {
            width: closed.control.resolution().width(),
            height: closed.control.resolution().height(),
        }),
        defaults: RecognitionDefaults {
            template_threshold: recognition.template_threshold,
            color_max_distance: recognition.color_max_distance,
            match_metric: match recognition.match_metric {
                AdmittedMatchMetric::CcorrNormed => RecognitionMatchMetric::CcorrNormed,
                AdmittedMatchMetric::CcoeffNormed => RecognitionMatchMetric::CcoeffNormed,
            },
        },
        targets: closed
            .targets
            .iter()
            .map(|(key, target)| canonical_recognition_target(key, target))
            .collect(),
    };
    let resolver = Arc::new(CanonicalAssetResolver {
        assets: closed
            .asset_bytes
            .iter()
            .map(|(key, bytes)| (key.as_str().to_string(), Arc::clone(bytes)))
            .collect(),
    });
    let evaluator = RecognitionEvaluator::with_asset_resolver(pack, resolver).map_err(|error| {
        AdmissionError::with_detail("admission_recognition_invalid", error.to_string())
    })?;
    let page_set = PageSet {
        schema_version: recognition.page_schema_version.clone(),
        pages: closed
            .pages
            .values()
            .map(|page| PageDefinition {
                id: page.detector_id.clone(),
                required: page.required.iter().map(ToString::to_string).collect(),
                any_of: page
                    .any_of
                    .iter()
                    .map(|group| group.iter().map(ToString::to_string).collect())
                    .collect(),
                optional: page.optional.iter().map(ToString::to_string).collect(),
                forbidden: page.forbidden.iter().map(ToString::to_string).collect(),
            })
            .collect(),
    };
    let detector = PageDetector::new(page_set).map_err(|error| {
        AdmissionError::with_detail("admission_recognition_invalid", error.to_string())
    })?;
    detector.validate(&evaluator).map_err(|error| {
        AdmissionError::with_detail("admission_recognition_invalid", error.to_string())
    })?;
    Ok((evaluator, detector))
}

fn canonical_recognition_target(key: &TargetKey, target: &AdmittedTarget) -> RecognitionTarget {
    match target {
        AdmittedTarget::Template {
            asset,
            region,
            threshold,
            method,
            mask,
            rect_move,
            color_check,
            click,
        } => RecognitionTarget::Template(TemplateTarget {
            id: key.as_str().to_string(),
            template_path: asset.as_str().to_string(),
            region: match region {
                Some(rect) => PackRegion::Rect(pack_rect(*rect)),
                None => PackRegion::Keyword("full_frame".to_string()),
            },
            threshold: *threshold,
            method: match method {
                AdmittedRecognitionMethod::Ncc => RecognitionMethod::Ncc,
                AdmittedRecognitionMethod::RgbCount => RecognitionMethod::RgbCount,
                AdmittedRecognitionMethod::HsvCount => RecognitionMethod::HsvCount,
            },
            mask: mask.as_ref().map(|mask| match mask {
                AdmittedRecognitionMask::Range { lower, upper } => RecognitionMask::Range {
                    lower: *lower,
                    upper: *upper,
                },
                AdmittedRecognitionMask::Bitmap { asset } => RecognitionMask::Bitmap {
                    path: asset.as_str().to_string(),
                },
            }),
            rect_move: rect_move.map(pack_rect),
            color_check: color_check.as_ref().map(|check| ColorCheck {
                region: pack_rect(check.region),
                expected: check.expected,
            }),
            click: click.map(pack_rect),
        }),
        AdmittedTarget::Color {
            region,
            expected,
            click,
        } => RecognitionTarget::Color(ColorTarget {
            id: key.as_str().to_string(),
            region: pack_rect(*region),
            expected: *expected,
            click: click.map(pack_rect),
        }),
        AdmittedTarget::ClickOnly { click } => RecognitionTarget::ClickOnly(ClickOnlyTarget {
            id: key.as_str().to_string(),
            click: pack_rect(*click),
        }),
    }
}

fn pack_rect(rect: BoundedRect) -> PackRect {
    PackRect {
        x: rect.x(),
        y: rect.y(),
        width: rect.width(),
        height: rect.height(),
    }
}

#[derive(Serialize)]
struct CanonicalAssetProjection<'a> {
    key: &'a AssetKey,
    sha256: String,
}

#[derive(Serialize)]
struct CanonicalSemanticProjection<'a> {
    schema_version: &'static str,
    control: &'a AdmittedControl,
    entry_task: &'a AdmittedTask,
    tasks: Vec<(&'a TaskKey, &'a AdmittedTask)>,
    navigation: &'a Option<AdmittedNavigation>,
    recognition: &'a AdmittedRecognitionMetadata,
    pages: Vec<(&'a PageKey, &'a AdmittedPage)>,
    targets: Vec<(&'a TargetKey, &'a AdmittedTarget)>,
    assets: Vec<CanonicalAssetProjection<'a>>,
}

fn canonical_semantic_fingerprint(closed: &ClosedPackage) -> AdmissionResult<String> {
    const DOMAIN: &[u8] = b"ActingCommand canonical executable package v1\0";
    if closed.assets.len() != closed.asset_bytes.len() {
        return Err(AdmissionError::new("admission_asset_closure"));
    }
    let assets = closed
        .assets
        .iter()
        .map(|(key, expected_hash)| {
            let bytes = closed
                .asset_bytes
                .get(key)
                .ok_or_else(|| AdmissionError::new("admission_asset_closure"))?;
            let sha256 = Sha256Hash::digest(bytes).to_string();
            if &sha256 != expected_hash {
                return Err(AdmissionError::with_detail(
                    "admission_asset_closure",
                    format!("canonical asset '{key}' bytes do not match the closed digest"),
                ));
            }
            Ok(CanonicalAssetProjection { key, sha256 })
        })
        .collect::<AdmissionResult<Vec<_>>>()?;
    let projection = CanonicalSemanticProjection {
        schema_version: "actingcommand.canonical-executable-package.v1",
        control: &closed.control,
        entry_task: &closed.entry_task,
        tasks: closed.tasks.iter().collect(),
        navigation: &closed.navigation,
        recognition: &closed.recognition,
        pages: closed.pages.iter().collect(),
        targets: closed.targets.iter().collect(),
        assets,
    };
    let encoded = serde_json::to_vec(&projection).map_err(|error| {
        AdmissionError::with_detail(
            "admission_canonicalization_failed",
            format!("failed to serialize canonical semantic projection: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

fn close_control(raw: &RawControlV1) -> AdmissionResult<AdmittedControl> {
    let package_id = non_empty(&raw.package_id, "admission_identity_invalid", "package_id")?;
    let game = non_empty(&raw.game, "admission_identity_invalid", "game")?;
    let server = non_empty(&raw.server, "admission_identity_invalid", "server")?;
    let entry_task = TaskKey::parse(raw.entry_task_id.clone(), "admission_identity_invalid")?;
    let execution_mode = ExecutionMode::parse(&raw.execution_mode)?;
    let resolution = PackageResolution::new(raw.resolution.width, raw.resolution.height)?;
    if raw
        .resource_root
        .as_deref()
        .is_some_and(|value| value != "resources")
    {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }

    let capture_interval_ms = bounded_positive_or_default(
        raw.capture_interval_ms,
        DEFAULT_CAPTURE_INTERVAL_MS,
        MAX_CAPTURE_INTERVAL_MS,
        "admission_control_invalid",
    )?;
    let timeout_ms = bounded_positive_or_default(
        raw.timeout_ms,
        DEFAULT_TASK_TIMEOUT_MS,
        MAX_TASK_TIMEOUT_MS,
        "admission_control_invalid",
    )?;
    let step_timeout_ms = bounded_positive_or_default(
        raw.step_timeout_ms,
        DEFAULT_STEP_TIMEOUT_MS,
        MAX_STEP_TIMEOUT_MS,
        "admission_control_invalid",
    )?;
    let max_steps = match raw.max_steps {
        Some(value) if (1..=MAX_STEPS).contains(&value) => value,
        Some(_) => return Err(AdmissionError::new("admission_control_invalid")),
        None => DEFAULT_MAX_STEPS,
    };
    let capture_backend = raw
        .capture_backend
        .as_deref()
        .map(normalize_capture_backend)
        .transpose()?
        .map(str::to_string);
    let frame_store = close_frame_store(raw.frame_store.clone())?;

    Ok(AdmittedControl {
        package_id,
        execution_mode,
        game,
        server,
        resolution,
        entry_task,
        capture_interval_ms,
        timeout_ms,
        step_timeout_ms,
        max_steps,
        stop_on_error: raw.stop_on_error,
        stop_on_confirmation: raw.stop_on_confirmation.unwrap_or(true),
        allow_placeholder_coords: raw.allow_placeholder_coords.unwrap_or(false),
        output: raw.output.clone().map(OpaqueMetadata::new),
        capture_backend,
        frame_store,
        producer_present: raw.producer.is_some(),
        trusted_execution_present: raw.trusted_execution.is_some(),
    })
}

fn close_frame_store(raw: RawFrameStoreControl) -> AdmissionResult<FrameStoreSettings> {
    if raw
        .similarity_threshold
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(AdmissionError::new("admission_control_invalid"));
    }
    for value in [
        raw.tier1_ratio,
        raw.tier2_ratio,
        raw.tier3_ratio,
        raw.hysteresis_ratio,
    ]
    .into_iter()
    .flatten()
    {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(AdmissionError::new("admission_control_invalid"));
        }
    }
    if raw.max_mem_bytes == Some(0) || raw.flush_workspace_reserve_bytes == Some(0) {
        return Err(AdmissionError::new("admission_control_invalid"));
    }
    Ok(FrameStoreSettings {
        similarity_threshold: raw.similarity_threshold,
        tier1_ratio: raw.tier1_ratio,
        tier2_ratio: raw.tier2_ratio,
        tier3_ratio: raw.tier3_ratio,
        hysteresis_ratio: raw.hysteresis_ratio,
        max_mem_bytes: raw.max_mem_bytes,
        os_reserve_bytes: raw.os_reserve_bytes,
        flush_workspace_reserve_bytes: raw.flush_workspace_reserve_bytes,
    })
}

fn normalize_capture_backend(value: &str) -> AdmissionResult<&'static str> {
    match value {
        "auto" => Ok("auto"),
        "auto-fastest" | "auto_fastest" => Ok("auto-fastest"),
        "adb" | "adb_screencap" | "screencap" => Ok("adb"),
        "droidcast_raw" | "droidcast" => Ok("droidcast_raw"),
        "nemu_ipc" | "nemu" => Ok("nemu_ipc"),
        _ => Err(AdmissionError::new("admission_control_invalid")),
    }
}

type ClosedRecognition = (
    AdmittedRecognitionMetadata,
    BTreeMap<TargetKey, AdmittedTarget>,
    BTreeMap<AssetKey, String>,
);

fn close_recognition(
    raw: &RawRecognitionDocument,
    page_schema_version: &str,
    control: &AdmittedControl,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
) -> AdmissionResult<ClosedRecognition> {
    validate_generated_metadata(
        &raw.schema_version,
        raw.converter_schema_version.as_deref(),
        raw.generated,
        raw.generated_by.as_deref(),
    )?;
    if raw
        .game
        .as_deref()
        .is_some_and(|game| game != control.game())
        || raw
            .server
            .as_deref()
            .is_some_and(|server| server != control.server())
    {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    if raw
        .locale
        .as_deref()
        .is_some_and(|value| value.trim().is_empty() || value.trim() != value)
    {
        return Err(AdmissionError::new("admission_identity_invalid"));
    }
    let coordinate_space = raw
        .coordinate_space
        .ok_or_else(|| AdmissionError::new("admission_resolution_invalid"))?;
    if coordinate_space.width != control.resolution().width()
        || coordinate_space.height != control.resolution().height()
    {
        return Err(AdmissionError::new("admission_resolution_invalid"));
    }
    let template_threshold = raw
        .defaults
        .template_threshold
        .unwrap_or(DEFAULT_TEMPLATE_THRESHOLD);
    let color_max_distance = raw
        .defaults
        .color_max_distance
        .unwrap_or(DEFAULT_COLOR_MAX_DISTANCE);
    if !template_threshold.is_finite()
        || !(0.0..=1.0).contains(&template_threshold)
        || !color_max_distance.is_finite()
        || color_max_distance < 0.0
    {
        return Err(AdmissionError::new("admission_recognition_invalid"));
    }
    let match_metric = match raw
        .defaults
        .match_metric
        .unwrap_or(RawMatchMetric::CcorrNormed)
    {
        RawMatchMetric::CcorrNormed => AdmittedMatchMetric::CcorrNormed,
        RawMatchMetric::CcoeffNormed => AdmittedMatchMetric::CcoeffNormed,
    };
    let metadata = AdmittedRecognitionMetadata {
        schema_version: raw.schema_version.clone(),
        page_schema_version: page_schema_version.to_string(),
        locale: raw.locale.clone(),
        template_threshold,
        color_max_distance,
        match_metric,
    };

    let mut targets = BTreeMap::new();
    let mut assets = BTreeMap::new();
    for target in &raw.targets {
        let (key, admitted) = match target {
            RawRecognitionTarget::Template {
                id,
                template_path,
                region,
                threshold,
                method,
                mask,
                rect_move,
                color_check,
                click,
            } => {
                if threshold
                    .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
                {
                    return Err(AdmissionError::new("admission_recognition_invalid"));
                }
                let key = TargetKey::parse(id.clone(), "admission_target_closure")?;
                let asset =
                    close_resource_asset(template_path, entries, resource_root, &mut assets)?;
                let region = match region {
                    RawRegion::Rect(rect) => {
                        Some(BoundedRect::from_raw(*rect, control.resolution())?)
                    }
                    RawRegion::Keyword(value) if value == "full_frame" => None,
                    RawRegion::Keyword(_) => {
                        return Err(AdmissionError::new("admission_recognition_invalid"));
                    }
                };
                let method = match method.unwrap_or(RawRecognitionMethod::Ncc) {
                    RawRecognitionMethod::Ncc => AdmittedRecognitionMethod::Ncc,
                    RawRecognitionMethod::RgbCount => AdmittedRecognitionMethod::RgbCount,
                    RawRecognitionMethod::HsvCount => AdmittedRecognitionMethod::HsvCount,
                };
                let mask = match mask {
                    None => None,
                    Some(RawRecognitionMask::Range { lower, upper }) if lower <= upper => {
                        Some(AdmittedRecognitionMask::Range {
                            lower: *lower,
                            upper: *upper,
                        })
                    }
                    Some(RawRecognitionMask::Range { .. }) => {
                        return Err(AdmissionError::new("admission_recognition_invalid"));
                    }
                    Some(RawRecognitionMask::Bitmap { path }) => {
                        Some(AdmittedRecognitionMask::Bitmap {
                            asset: close_resource_asset(path, entries, resource_root, &mut assets)?,
                        })
                    }
                };
                let rect_move = rect_move
                    .map(|rect| BoundedRect::from_raw(rect, control.resolution()))
                    .transpose()?;
                let color_check = color_check
                    .map(|check| {
                        Ok(AdmittedColorCheck {
                            region: BoundedRect::from_raw(check.region, control.resolution())?,
                            expected: check.expected,
                        })
                    })
                    .transpose()?;
                let click = click
                    .map(|rect| BoundedRect::from_raw(rect, control.resolution()))
                    .transpose()?;
                (
                    key,
                    AdmittedTarget::Template {
                        asset,
                        region,
                        threshold: *threshold,
                        method,
                        mask,
                        rect_move,
                        color_check,
                        click,
                    },
                )
            }
            RawRecognitionTarget::Color {
                id,
                region,
                expected,
                click,
            } => (
                TargetKey::parse(id.clone(), "admission_target_closure")?,
                AdmittedTarget::Color {
                    region: BoundedRect::from_raw(*region, control.resolution())?,
                    expected: *expected,
                    click: click
                        .map(|rect| BoundedRect::from_raw(rect, control.resolution()))
                        .transpose()?,
                },
            ),
            RawRecognitionTarget::ClickOnly { id, click } => (
                TargetKey::parse(id.clone(), "admission_target_closure")?,
                AdmittedTarget::ClickOnly {
                    click: BoundedRect::from_raw(*click, control.resolution())?,
                },
            ),
        };
        if targets.insert(key, admitted).is_some() {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
    }
    if targets.is_empty() {
        return Err(AdmissionError::new("admission_recognition_invalid"));
    }
    Ok((metadata, targets, assets))
}

fn close_pages(
    raw: &RawPageDocument,
    control: &AdmittedControl,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<BTreeMap<PageKey, AdmittedPage>> {
    validate_generated_metadata(
        &raw.schema_version,
        raw.converter_schema_version.as_deref(),
        raw.generated,
        raw.generated_by.as_deref(),
    )?;
    let mut pages = BTreeMap::new();
    for page in &raw.pages {
        let key = PageKey::parse(control.game(), &page.id)?;
        let required = close_target_list(&page.required, targets)?;
        let optional = close_target_list(&page.optional, targets)?;
        let forbidden = close_target_list(&page.forbidden, targets)?;
        let mut any_of = page
            .any_of
            .iter()
            .map(|group| {
                let group = close_target_list(group, targets)?;
                if group.is_empty() {
                    return Err(AdmissionError::new("admission_page_closure"));
                }
                Ok(group)
            })
            .collect::<AdmissionResult<Vec<_>>>()?;
        any_of.sort();
        if required.is_empty() && any_of.is_empty() {
            return Err(AdmissionError::new("admission_page_closure"));
        }
        if required.iter().any(|target| forbidden.contains(target)) {
            return Err(AdmissionError::new("admission_page_closure"));
        }
        let admitted = AdmittedPage {
            detector_id: key.qualified(),
            required,
            any_of,
            optional,
            forbidden,
        };
        if pages.insert(key, admitted).is_some() {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
    }
    if pages.is_empty() {
        return Err(AdmissionError::new("admission_page_closure"));
    }
    Ok(pages)
}

fn close_target_list(
    raw: &[String],
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<Vec<TargetKey>> {
    let mut closed = raw
        .iter()
        .map(|id| {
            let key = TargetKey::parse(id.clone(), "admission_target_closure")?;
            if !targets.contains_key(&key) {
                return Err(AdmissionError::with_detail(
                    "admission_missing_reference",
                    format!("target '{key}' is not packaged"),
                ));
            }
            Ok(key)
        })
        .collect::<AdmissionResult<Vec<_>>>()?;
    closed.sort();
    if closed.windows(2).any(|items| items[0] == items[1]) {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    Ok(closed)
}

fn close_resource_asset(
    relative: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    assets: &mut BTreeMap<AssetKey, String>,
) -> AdmissionResult<AssetKey> {
    validate_relative_ref(relative).map_err(|_| AdmissionError::new("admission_asset_closure"))?;
    let key = AssetKey::parse(relative.to_string(), "admission_asset_closure")?;
    let entry_path = prefixed_path(resource_root, relative);
    let bytes = entries.get(&entry_path).ok_or_else(|| {
        AdmissionError::with_detail(
            "admission_missing_reference",
            format!("asset '{relative}' is not packaged"),
        )
    })?;
    let hash = Sha256Hash::digest(bytes).to_string();
    if let Some(previous) = assets.insert(key.clone(), hash.clone())
        && previous != hash
    {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    Ok(key)
}

fn close_operation_asset(
    task: &TaskKey,
    relative: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    assets: &mut BTreeMap<AssetKey, String>,
) -> AdmissionResult<AssetKey> {
    validate_relative_ref(relative).map_err(|_| AdmissionError::new("admission_asset_closure"))?;
    let local = format!("operations/{task}/{relative}");
    if entries.contains_key(&prefixed_path(resource_root, &local)) {
        close_resource_asset(&local, entries, resource_root, assets)
    } else {
        close_resource_asset(relative, entries, resource_root, assets)
    }
}

fn non_empty(value: &str, code: &'static str, label: &str) -> AdmissionResult<String> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(AdmissionError::with_detail(
            code,
            format!("{label} must be a trimmed non-empty string"),
        ));
    }
    Ok(value.to_string())
}

fn bounded_positive_or_default(
    value: Option<u64>,
    default: u64,
    maximum: u64,
    code: &'static str,
) -> AdmissionResult<u64> {
    match value {
        Some(value) if (1..=maximum).contains(&value) => Ok(value),
        Some(_) => Err(AdmissionError::new(code)),
        None => Ok(default),
    }
}

fn validate_opaque_object(value: Option<&Value>, label: &str) -> AdmissionResult<()> {
    if value.is_some_and(|value| !value.is_object()) {
        return Err(AdmissionError::with_detail(
            "admission_operation_invalid",
            format!("{label} must be an object"),
        ));
    }
    Ok(())
}

fn validate_optional_unit_interval(value: Option<f32>, label: &str) -> AdmissionResult<()> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(AdmissionError::with_detail(
            "admission_operation_invalid",
            format!("{label} must be finite and between zero and one"),
        ));
    }
    Ok(())
}

fn validate_generated_metadata(
    schema_version: &str,
    converter_schema_version: Option<&str>,
    generated: Option<bool>,
    generated_by: Option<&str>,
) -> AdmissionResult<()> {
    match (converter_schema_version, generated, generated_by) {
        (None, None, None) => Ok(()),
        (Some(converter), Some(true), Some(generator))
            if converter == schema_version
                && !generator.trim().is_empty()
                && generator.trim() == generator =>
        {
            Ok(())
        }
        _ => Err(AdmissionError::new("admission_generated_metadata_invalid")),
    }
}

fn close_task_region(
    region: Option<&RawTaskRegion>,
    control: &AdmittedControl,
) -> AdmissionResult<Option<BoundedRect>> {
    match region {
        None | Some(RawTaskRegion::FullFrame) => Ok(None),
        Some(RawTaskRegion::Rect { rect }) => {
            BoundedRect::from_raw(*rect, control.resolution()).map(Some)
        }
        Some(RawTaskRegion::Auto) => Err(AdmissionError::new("admission_recognition_invalid")),
    }
}

fn validate_authoring_target<'a>(
    key: &TargetKey,
    expected_kind: AdmittedTargetKind,
    expected_asset: Option<&AssetKey>,
    targets: &'a BTreeMap<TargetKey, AdmittedTarget>,
    seen: &mut BTreeSet<TargetKey>,
) -> AdmissionResult<&'a AdmittedTarget> {
    if !seen.insert(key.clone()) {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    let target = targets
        .get(key)
        .ok_or_else(|| AdmissionError::new("admission_target_closure"))?;
    if target.kind() != expected_kind
        || expected_asset.is_some_and(|asset| target.template_asset() != Some(asset))
    {
        return Err(AdmissionError::new("admission_target_closure"));
    }
    Ok(target)
}

fn close_task(
    raw: &RawTaskDocument,
    control: &AdmittedControl,
    pages: &BTreeMap<PageKey, AdmittedPage>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    assets: &mut BTreeMap<AssetKey, String>,
) -> AdmissionResult<AdmittedTask> {
    let key = TaskKey::parse(raw.task_id.clone(), "admission_identity_invalid")?;
    if raw.game != control.game()
        || (!raw.server_scope.is_empty()
            && !raw
                .server_scope
                .iter()
                .any(|server| server == control.server()))
        || raw.coordinate_space.width != control.resolution().width()
        || raw.coordinate_space.height != control.resolution().height()
    {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    for server in &raw.server_scope {
        non_empty(server, "admission_identity_invalid", "server_scope entry")?;
    }
    if let Some(locale) = &raw.locale {
        non_empty(locale, "admission_identity_invalid", "task locale")?;
    }
    validate_opaque_object(raw.provenance.as_ref(), "task provenance")?;

    let defaults = close_operation_defaults(raw.defaults)?;
    let mut anchors = Vec::with_capacity(raw.anchors.len());
    let mut anchor_ids = BTreeSet::new();
    let mut authoring_target_ids = BTreeSet::new();
    for anchor in &raw.anchors {
        let id = non_empty(
            &anchor.id,
            "admission_identity_invalid",
            "operation anchor id",
        )?;
        if !anchor_ids.insert(id.clone()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        let asset = close_operation_asset(&key, &anchor.template, entries, resource_root, assets)?;
        let region = close_task_region(anchor.region.as_ref(), control)?;
        validate_optional_unit_interval(anchor.threshold, "operation anchor threshold")?;
        validate_opaque_object(anchor.color_check.as_ref(), "operation anchor color_check")?;
        validate_opaque_object(anchor.provenance.as_ref(), "operation anchor provenance")?;
        let target_key =
            TargetKey::parse(format!("page/{}", anchor.id), "admission_target_closure")?;
        let target = validate_authoring_target(
            &target_key,
            AdmittedTargetKind::Template,
            Some(&asset),
            targets,
            &mut authoring_target_ids,
        )?;
        if let AdmittedTarget::Template {
            region: target_region,
            threshold: target_threshold,
            ..
        } = target
            && (anchor.region.is_some() && region != *target_region
                || anchor.threshold.is_some() && anchor.threshold != *target_threshold)
        {
            return Err(AdmissionError::new("admission_target_closure"));
        }
        anchors.push(AdmittedAnchor { id, asset });
    }
    anchors.sort_by(|left, right| left.id.cmp(&right.id));

    for probe in &raw.color_probes {
        let region = close_task_region(Some(&probe.region), control)?
            .ok_or_else(|| AdmissionError::new("admission_recognition_invalid"))?;
        validate_opaque_object(probe.provenance.as_ref(), "color probe provenance")?;
        let target_key = TargetKey::parse(probe.id.clone(), "admission_target_closure")?;
        let target = validate_authoring_target(
            &target_key,
            AdmittedTargetKind::Color,
            None,
            targets,
            &mut authoring_target_ids,
        )?;
        if !matches!(
            target,
            AdmittedTarget::Color {
                region: target_region,
                expected,
                ..
            } if *target_region == region && *expected == probe.expected
        ) {
            return Err(AdmissionError::new("admission_target_closure"));
        }
    }
    for template in &raw.verify_templates {
        let region = close_task_region(Some(&template.region), control)?;
        validate_optional_unit_interval(template.threshold, "verify template threshold")?;
        validate_opaque_object(template.provenance.as_ref(), "verify template provenance")?;
        let asset =
            close_operation_asset(&key, &template.template, entries, resource_root, assets)?;
        let target_key = TargetKey::parse(template.id.clone(), "admission_target_closure")?;
        let target = validate_authoring_target(
            &target_key,
            AdmittedTargetKind::Template,
            Some(&asset),
            targets,
            &mut authoring_target_ids,
        )?;
        if let AdmittedTarget::Template {
            region: target_region,
            threshold: target_threshold,
            ..
        } = target
            && (region != *target_region
                || template.threshold.is_some() && template.threshold != *target_threshold)
        {
            return Err(AdmissionError::new("admission_target_closure"));
        }
    }

    let entry_page = raw
        .entry_page
        .as_deref()
        .map(|page| close_exact_page(control.game(), page, pages))
        .transpose()?;
    let target_page = raw
        .target_page
        .as_deref()
        .map(|page| close_exact_page(control.game(), page, pages))
        .transpose()?;
    let mut error_pages = raw
        .error_pages
        .iter()
        .map(|page| close_exact_page(control.game(), page, pages))
        .collect::<AdmissionResult<Vec<_>>>()?;
    error_pages.sort();
    if error_pages.windows(2).any(|items| items[0] == items[1]) {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    validate_page_rules(&raw.page_rules, control.game(), pages, targets)?;

    let recovery = raw.recovery.as_ref().map(close_recovery).transpose()?;
    if raw.max_task_retries == Some(0) {
        return Err(AdmissionError::new("admission_task_graph_invalid"));
    }
    let pause_on_exhausted = match raw.on_exhausted.as_deref() {
        None => false,
        Some("pause") => true,
        Some(_) => return Err(AdmissionError::new("admission_task_graph_invalid")),
    };

    let mut operations = Vec::with_capacity(raw.operations.len());
    let mut operation_ids = BTreeSet::new();
    for operation in &raw.operations {
        let operation = close_operation(
            &key,
            operation,
            control,
            pages,
            targets,
            entries,
            resource_root,
            assets,
        )?;
        if !operation_ids.insert(operation.key().operation().to_string()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        operations.push(operation);
    }
    operations.sort_by(|left, right| left.key().cmp(right.key()));

    Ok(AdmittedTask {
        key,
        schema_version: raw.schema_version.clone(),
        goal: raw.goal.clone(),
        defaults,
        anchors,
        entry_page,
        target_page,
        error_pages,
        recovery,
        max_task_retries: raw.max_task_retries,
        pause_on_exhausted,
        operations,
    })
}

fn close_operation_defaults(
    raw: RawOperationDefaults,
) -> AdmissionResult<AdmittedOperationDefaults> {
    let _match_metric = raw.match_metric;
    let template_threshold = raw.template_threshold.unwrap_or(DEFAULT_TEMPLATE_THRESHOLD);
    if !template_threshold.is_finite() || !(0.0..=1.0).contains(&template_threshold) {
        return Err(AdmissionError::new("admission_operation_invalid"));
    }
    if raw
        .color_max_distance
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(AdmissionError::new("admission_operation_invalid"));
    }
    validate_optional_positive(raw.timeout_ms, MAX_TASK_TIMEOUT_MS)?;
    validate_optional_positive(raw.max_attempts.map(u64::from), u64::from(MAX_STEPS))?;
    validate_optional_positive(raw.retry_interval_ms, MAX_TASK_TIMEOUT_MS)?;
    for value in [
        raw.pre_delay_ms,
        raw.post_delay_ms,
        raw.pre_wait_freezes_ms,
        raw.post_wait_freezes_ms,
    ] {
        validate_optional_nonnegative(value, MAX_TASK_TIMEOUT_MS)?;
    }
    Ok(AdmittedOperationDefaults {
        template_threshold,
        color_max_distance: raw.color_max_distance,
        timeout_ms: raw.timeout_ms,
        max_attempts: raw.max_attempts,
        retry_interval_ms: raw.retry_interval_ms,
        pre_delay_ms: raw.pre_delay_ms,
        post_delay_ms: raw.post_delay_ms,
        pre_wait_freezes_ms: raw.pre_wait_freezes_ms,
        post_wait_freezes_ms: raw.post_wait_freezes_ms,
    })
}

#[allow(clippy::too_many_arguments)]
fn close_operation(
    task: &TaskKey,
    raw: &RawOperation,
    control: &AdmittedControl,
    pages: &BTreeMap<PageKey, AdmittedPage>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    assets: &mut BTreeMap<AssetKey, String>,
) -> AdmissionResult<AdmittedOperation> {
    let key = OperationKey::new(task.clone(), raw.id.clone())?;
    let from = close_page_selector(control.game(), &raw.from, pages)?;
    let to = raw
        .to
        .as_deref()
        .map(|page| close_exact_page(control.game(), page, pages))
        .transpose()?;
    let guard = raw
        .guard
        .as_ref()
        .map(|guard| {
            close_guard(
                task,
                guard,
                &from,
                control,
                pages,
                targets,
                entries,
                resource_root,
                assets,
            )
        })
        .transpose()?;
    match (guard.is_some(), raw.unguarded_trusted_coordinate) {
        (true, true) | (false, false) => {
            return Err(AdmissionError::new("admission_guard_invalid"));
        }
        (true, false) | (false, true) => {}
    }
    let action = close_operation_action(&raw.click, guard.as_ref(), control, targets)?;
    let verify_template = raw
        .verify_template
        .as_deref()
        .map(|asset| close_operation_asset(task, asset, entries, resource_root, assets))
        .transpose()?;
    let expect_after = raw
        .expect_after
        .as_ref()
        .map(|expectation| {
            validate_optional_positive(expectation.timeout_ms, MAX_TASK_TIMEOUT_MS)?;
            validate_optional_positive(expectation.interval_ms, MAX_TASK_TIMEOUT_MS)?;
            Ok(AdmittedExpectation {
                page: close_exact_page(control.game(), &expectation.page_id, pages)?,
                timeout_ms: expectation.timeout_ms,
                interval_ms: expectation.interval_ms,
            })
        })
        .transpose()?;
    if let (Some(to), Some(expectation)) = (&to, &expect_after)
        && expectation.page() != to
    {
        return Err(AdmissionError::new("admission_page_closure"));
    }

    validate_optional_positive(raw.timeout_ms, MAX_TASK_TIMEOUT_MS)?;
    validate_optional_positive(raw.max_attempts.map(u64::from), u64::from(MAX_STEPS))?;
    validate_optional_positive(raw.retry_interval_ms, MAX_TASK_TIMEOUT_MS)?;
    for value in [
        raw.pre_delay_ms,
        raw.post_delay_ms,
        raw.pre_wait_freezes_ms,
        raw.post_wait_freezes_ms,
    ] {
        validate_optional_nonnegative(value, MAX_TASK_TIMEOUT_MS)?;
    }
    if raw
        .effect
        .as_deref()
        .is_some_and(|effect| effect != "navigation_only")
    {
        return Err(AdmissionError::new("admission_operation_invalid"));
    }
    let on_error = raw
        .on_error
        .as_ref()
        .map(|task| TaskKey::parse(task.clone(), "admission_task_graph_invalid"))
        .transpose()?;
    let consumes = close_string_set(&raw.consumes, "operation consumes")?;
    let produces = close_string_set(&raw.produces, "operation produces")?;
    let navigation_only = raw.effect.as_deref() == Some("navigation_only")
        || (to.is_some() && consumes.is_empty() && produces.is_empty());

    Ok(AdmittedOperation {
        key,
        purpose: raw.purpose.clone(),
        from,
        to,
        action,
        verify_template,
        expect_after,
        timeout_ms: raw.timeout_ms,
        max_attempts: raw.max_attempts,
        retry_interval_ms: raw.retry_interval_ms,
        pre_delay_ms: raw.pre_delay_ms,
        post_delay_ms: raw.post_delay_ms,
        pre_wait_freezes_ms: raw.pre_wait_freezes_ms,
        post_wait_freezes_ms: raw.post_wait_freezes_ms,
        retryable: raw.retryable,
        navigation_only,
        effect_capability: if raw.destructive {
            AdmittedEffectCapability::Destructive
        } else {
            AdmittedEffectCapability::NavigationOnly
        },
        on_error,
        guard,
        unguarded_trusted_coordinate: raw.unguarded_trusted_coordinate,
        consumes,
        produces,
        verified_live: raw.verified_live,
        provenance: raw.provenance.clone().map(OpaqueMetadata::new),
    })
}

#[allow(clippy::too_many_arguments)]
fn close_guard(
    task: &TaskKey,
    raw: &RawOperationGuard,
    from: &PageSelector,
    control: &AdmittedControl,
    pages: &BTreeMap<PageKey, AdmittedPage>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
    entries: &BTreeMap<String, Vec<u8>>,
    resource_root: &str,
    assets: &mut BTreeMap<AssetKey, String>,
) -> AdmissionResult<AdmittedGuard> {
    let page = close_exact_page(control.game(), &raw.page_id, pages)?;
    if !from.matches(&page) {
        return Err(AdmissionError::new("admission_guard_invalid"));
    }
    let target = TargetKey::parse(raw.target_id.clone(), "admission_target_closure")?;
    let target_definition = targets.get(&target).ok_or_else(|| {
        AdmissionError::with_detail(
            "admission_missing_reference",
            format!("guard target '{target}' is not packaged"),
        )
    })?;
    let verification = match (&raw.verify_template, &raw.color_probe) {
        (Some(asset), None) if target_definition.kind() == AdmittedTargetKind::Template => {
            let asset = close_operation_asset(task, asset, entries, resource_root, assets)?;
            if target_definition.template_asset() != Some(&asset) {
                return Err(AdmissionError::new("admission_asset_closure"));
            }
            GuardVerification::Template { asset }
        }
        (None, Some(probe)) if target_definition.kind() == AdmittedTargetKind::Color => {
            let probe = TargetKey::parse(probe.clone(), "admission_target_closure")?;
            if probe != target {
                return Err(AdmissionError::new("admission_target_closure"));
            }
            GuardVerification::Color { probe }
        }
        _ => return Err(AdmissionError::new("admission_guard_invalid")),
    };
    Ok(AdmittedGuard {
        page,
        target,
        expected_rect: BoundedRect::from_raw(raw.expected_rect, control.resolution())?,
        verification,
    })
}

fn close_operation_action(
    raw: &RawAction,
    guard: Option<&AdmittedGuard>,
    control: &AdmittedControl,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<AdmittedAction> {
    match raw {
        RawAction::Point { x, y } => {
            let rect = close_click_rect(
                RawRect {
                    x: *x,
                    y: *y,
                    width: 1,
                    height: 1,
                },
                control,
            )?;
            Ok(AdmittedAction::Tap {
                point: rect.center(),
                rect,
            })
        }
        RawAction::Rect {
            x,
            y,
            width,
            height,
        }
        | RawAction::SpecificRect {
            x,
            y,
            width,
            height,
        } => {
            let rect = close_click_rect(
                RawRect {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                },
                control,
            )?;
            Ok(AdmittedAction::Tap {
                point: rect.center(),
                rect,
            })
        }
        RawAction::LongPress { x, y, duration_ms } | RawAction::LongTap { x, y, duration_ms } => {
            Ok(AdmittedAction::LongTap {
                point: close_click_point(*x, *y, control)?,
                duration: InputDuration::new(*duration_ms)?,
            })
        }
        RawAction::Drag {
            from_rect,
            to_rect,
            duration_ms,
        } => {
            let from_rect = close_click_rect(*from_rect, control)?;
            let to_rect = close_click_rect(*to_rect, control)?;
            Ok(AdmittedAction::Drag {
                from: from_rect.center(),
                to: to_rect.center(),
                from_rect,
                to_rect,
                duration: InputDuration::new(*duration_ms)?,
            })
        }
        RawAction::Target { target_id, offset } => close_target_action(
            target_id.as_deref(),
            offset.as_ref(),
            TargetTapMode::Deterministic,
            control.resolution(),
            guard,
            targets,
        ),
        RawAction::TargetCenter { target_id, offset } => close_target_action(
            target_id.as_deref(),
            offset.as_ref(),
            TargetTapMode::Center,
            control.resolution(),
            guard,
            targets,
        ),
        RawAction::Offset { target_id, offset } => close_target_action(
            target_id.as_deref(),
            Some(offset),
            TargetTapMode::Deterministic,
            control.resolution(),
            guard,
            targets,
        ),
    }
}

fn close_target_action(
    raw_target: Option<&str>,
    raw_offset: Option<&RawRect>,
    mode: TargetTapMode,
    resolution: PackageResolution,
    guard: Option<&AdmittedGuard>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<AdmittedAction> {
    let guard = guard.ok_or_else(|| AdmissionError::new("admission_guard_invalid"))?;
    if !matches!(guard.verification(), GuardVerification::Template { .. }) {
        return Err(AdmissionError::new("admission_guard_invalid"));
    }
    let target = raw_target
        .map(|value| TargetKey::parse(value.to_string(), "admission_target_closure"))
        .transpose()?
        .unwrap_or_else(|| guard.target().clone());
    if target != *guard.target()
        || targets.get(&target).map(AdmittedTarget::kind) != Some(AdmittedTargetKind::Template)
    {
        return Err(AdmissionError::new("admission_target_closure"));
    }
    Ok(AdmittedAction::TargetTap {
        target,
        mode,
        offset: raw_offset
            .copied()
            .map(|offset| TargetOffset::new(offset, resolution))
            .transpose()?,
    })
}

fn close_click_point(x: i32, y: i32, control: &AdmittedControl) -> AdmissionResult<BoundedPoint> {
    let point = BoundedPoint::new(x, y, control.resolution())?;
    if !control.allow_placeholder_coords() && x == 0 && y == 0 {
        return Err(AdmissionError::new("admission_input_bounds_invalid"));
    }
    Ok(point)
}

fn close_click_rect(raw: RawRect, control: &AdmittedControl) -> AdmissionResult<BoundedRect> {
    let rect = BoundedRect::from_raw(raw, control.resolution())?;
    if !control.allow_placeholder_coords()
        && ((rect.x() == 0 && rect.y() == 0 && rect.width() == 1 && rect.height() == 1)
            || (rect.x() == 0
                && rect.y() == 0
                && rect.width() as u32 == control.resolution().width()
                && rect.height() as u32 == control.resolution().height()))
    {
        return Err(AdmissionError::new("admission_input_bounds_invalid"));
    }
    Ok(rect)
}

fn close_recovery(raw: &RawTaskRecovery) -> AdmissionResult<TaskKey> {
    match raw {
        RawTaskRecovery::Kind(kind) if kind == "return_home" => {
            TaskKey::parse("return_home", "admission_task_graph_invalid")
        }
        RawTaskRecovery::Config(config) if config.kind == "return_home" => TaskKey::parse(
            config
                .task_id
                .clone()
                .unwrap_or_else(|| "return_home".to_string()),
            "admission_task_graph_invalid",
        ),
        RawTaskRecovery::Kind(_) | RawTaskRecovery::Config(_) => {
            Err(AdmissionError::new("admission_task_graph_invalid"))
        }
    }
}

fn validate_page_rules(
    rules: &BTreeMap<String, RawPageRule>,
    game: &str,
    pages: &BTreeMap<PageKey, AdmittedPage>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<()> {
    for (page, rule) in rules {
        let key = close_exact_page(game, page, pages)?;
        let admitted = pages
            .get(&key)
            .ok_or_else(|| AdmissionError::new("admission_page_closure"))?;
        let required = close_target_list(&rule.required, targets)?;
        let optional = close_target_list(&rule.optional, targets)?;
        let forbidden = close_target_list(&rule.forbidden, targets)?;
        if required
            .iter()
            .any(|target| !admitted.required.contains(target))
            || optional
                .iter()
                .any(|target| !admitted.optional.contains(target))
            || forbidden
                .iter()
                .any(|target| !admitted.forbidden.contains(target))
        {
            return Err(AdmissionError::new("admission_page_closure"));
        }
        for group in &rule.any_of {
            let mut group = close_target_list(group, targets)?;
            group.sort();
            if !admitted.any_of.contains(&group) {
                return Err(AdmissionError::new("admission_page_closure"));
            }
        }
    }
    Ok(())
}

fn close_exact_page(
    game: &str,
    raw: &str,
    pages: &BTreeMap<PageKey, AdmittedPage>,
) -> AdmissionResult<PageKey> {
    let key = resolve_packaged_page(game, raw, pages)?;
    if !pages.contains_key(&key) {
        return Err(AdmissionError::with_detail(
            "admission_missing_reference",
            format!("page '{key}' is not packaged"),
        ));
    }
    Ok(key)
}

fn resolve_packaged_page(
    game: &str,
    raw: &str,
    pages: &BTreeMap<PageKey, AdmittedPage>,
) -> AdmissionResult<PageKey> {
    let parsed = PageKey::parse(game, raw);
    if let Ok(key) = &parsed
        && pages.contains_key(key)
    {
        return Ok(key.clone());
    }
    if raw != "any" && !raw.trim().is_empty() && raw == raw.trim() {
        let relative = PageKey {
            game: game.to_string(),
            page: raw.to_string(),
        };
        if pages.contains_key(&relative) {
            return Ok(relative);
        }
    }
    parsed
}

fn close_string_set(raw: &[String], label: &str) -> AdmissionResult<Vec<String>> {
    let mut values = raw
        .iter()
        .map(|value| non_empty(value, "admission_operation_invalid", label))
        .collect::<AdmissionResult<Vec<_>>>()?;
    values.sort();
    if values.windows(2).any(|items| items[0] == items[1]) {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    Ok(values)
}

fn validate_optional_positive(value: Option<u64>, maximum: u64) -> AdmissionResult<()> {
    if value.is_some_and(|value| value == 0 || value > maximum) {
        Err(AdmissionError::new("admission_operation_invalid"))
    } else {
        Ok(())
    }
}

fn validate_optional_nonnegative(value: Option<u64>, maximum: u64) -> AdmissionResult<()> {
    if value.is_some_and(|value| value > maximum) {
        Err(AdmissionError::new("admission_operation_invalid"))
    } else {
        Ok(())
    }
}

fn validate_task_graph(
    entry: &TaskKey,
    tasks: &BTreeMap<TaskKey, AdmittedTask>,
) -> AdmissionResult<()> {
    if !tasks.contains_key(entry) {
        return Err(AdmissionError::with_detail(
            "admission_missing_reference",
            format!("entry task '{entry}' is not packaged"),
        ));
    }
    let mut dependencies = BTreeMap::<TaskKey, BTreeSet<TaskKey>>::new();
    for (key, task) in tasks {
        let mut task_dependencies = BTreeSet::new();
        if let Some(recovery) = task.recovery() {
            task_dependencies.insert(recovery.clone());
        }
        for operation in task.operations() {
            if let Some(on_error) = operation.on_error() {
                task_dependencies.insert(on_error.clone());
            }
        }
        for dependency in &task_dependencies {
            if !tasks.contains_key(dependency) {
                return Err(AdmissionError::with_detail(
                    "admission_missing_reference",
                    format!("task '{key}' references missing task '{dependency}'"),
                ));
            }
        }
        dependencies.insert(key.clone(), task_dependencies);
    }

    // Traverse the entry graph to a fixed point before the global cycle check. Every discovered
    // recovery/on_error edge is already resolved to a TaskKey above.
    let mut reachable = BTreeSet::new();
    let mut pending = vec![entry.clone()];
    while let Some(task) = pending.pop() {
        if !reachable.insert(task.clone()) {
            continue;
        }
        if let Some(next) = dependencies.get(&task) {
            pending.extend(next.iter().cloned());
        }
    }

    // Kahn's algorithm rejects self-cycles and transitive cycles without recursive stack growth.
    let mut remaining = dependencies
        .iter()
        .map(|(task, next)| (task.clone(), next.len()))
        .collect::<BTreeMap<_, _>>();
    let mut reverse = BTreeMap::<TaskKey, Vec<TaskKey>>::new();
    for (task, next) in &dependencies {
        for dependency in next {
            reverse
                .entry(dependency.clone())
                .or_default()
                .push(task.clone());
        }
    }
    let mut ready = remaining
        .iter()
        .filter_map(|(task, count)| (*count == 0).then_some(task.clone()))
        .collect::<BTreeSet<_>>();
    let mut processed = 0_usize;
    while let Some(task) = ready.pop_first() {
        processed += 1;
        for dependent in reverse.get(&task).into_iter().flatten() {
            let count = remaining
                .get_mut(dependent)
                .ok_or_else(|| AdmissionError::new("admission_task_graph_invalid"))?;
            *count = count
                .checked_sub(1)
                .ok_or_else(|| AdmissionError::new("admission_task_graph_invalid"))?;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if processed != tasks.len() {
        return Err(AdmissionError::new("admission_recovery_cycle"));
    }
    Ok(())
}

fn close_navigation(
    raw: Option<&RawNavigationDocument>,
    control: &AdmittedControl,
    tasks: &BTreeMap<TaskKey, AdmittedTask>,
    pages: &BTreeMap<PageKey, AdmittedPage>,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<Option<AdmittedNavigation>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    validate_generated_metadata(
        &raw.schema_version,
        raw.converter_schema_version.as_deref(),
        raw.generated,
        raw.generated_by.as_deref(),
    )?;
    if raw.game != control.game() || raw.server != control.server() {
        return Err(AdmissionError::new("admission_identity_conflict"));
    }
    if raw.coordinate_space.is_some_and(|resolution| {
        resolution.width != control.resolution().width()
            || resolution.height != control.resolution().height()
    }) {
        return Err(AdmissionError::new("admission_resolution_invalid"));
    }

    let mut routes = Vec::with_capacity(raw.routes.len());
    let mut route_ids = BTreeSet::new();
    let mut route_operations = BTreeSet::new();
    for route in &raw.routes {
        let id = non_empty(&route.id, "admission_identity_invalid", "route id")?;
        if !route_ids.insert(id.clone()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        let from = close_page_selector(control.game(), &route.from_page, pages)?;
        let to = close_exact_page(control.game(), &route.to_page, pages)?;
        let action = close_navigation_action(&route.click, control, targets)?;
        let candidates = all_operations(tasks)
            .filter(|operation| {
                operation.key().operation() == id
                    && operation.from() == &from
                    && operation.to() == Some(&to)
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err(AdmissionError::with_detail(
                "admission_missing_reference",
                format!(
                    "route '{id}' must resolve to exactly one operation; resolved {}",
                    candidates.len()
                ),
            ));
        }
        let operation = candidates[0];
        if operation.action() != &action {
            return Err(AdmissionError::with_detail(
                "admission_navigation_action_mismatch",
                format!("route '{id}' action differs from operation"),
            ));
        }
        if !route_operations.insert(operation.key().clone()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        let source = match route.source.as_deref() {
            None | Some("") => None,
            Some(source) => Some(non_empty(
                source,
                "admission_identity_invalid",
                "route source",
            )?),
        };
        routes.push(AdmittedRoute {
            operation: operation.key().clone(),
            source,
        });
    }
    routes.sort_by(|left, right| left.operation.cmp(&right.operation));

    let mut page_operations = Vec::with_capacity(raw.page_operations.len());
    let mut page_operation_keys = BTreeSet::new();
    for action in &raw.page_operations {
        let key = OperationKey::new(
            TaskKey::parse(action.task_id.clone(), "admission_identity_invalid")?,
            action.id.clone(),
        )?;
        let page = close_exact_page(control.game(), &action.page, pages)?;
        let operation = find_operation(tasks, &key)?;
        if operation.from() != &PageSelector::Exact(page)
            || operation.to().is_some()
            || operation.action() != &close_navigation_action(&action.click, control, targets)?
        {
            return Err(AdmissionError::new("admission_navigation_action_mismatch"));
        }
        if !page_operation_keys.insert(key.clone()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        page_operations.push(key);
    }
    page_operations.sort();

    let mut destructive_regions = Vec::with_capacity(raw.destructive_actions.len());
    for destructive in &raw.destructive_actions {
        let has_operation_identity = destructive.task_id.is_some() || destructive.id.is_some();
        if has_operation_identity
            && (destructive.task_id.is_none()
                || destructive.id.is_none()
                || destructive.page.is_none())
        {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        let page = destructive
            .page
            .as_deref()
            .map(|page| close_page_selector(control.game(), page, pages))
            .transpose()?
            .unwrap_or(PageSelector::Any);
        let action = close_navigation_tap(&destructive.click, control)?;
        let rect = match &action {
            AdmittedAction::Tap { rect, .. } => *rect,
            _ => return Err(AdmissionError::new("admission_operation_invalid")),
        };
        let operation = match (&destructive.task_id, &destructive.id) {
            (Some(task), Some(id)) => {
                let key = OperationKey::new(
                    TaskKey::parse(task.clone(), "admission_identity_invalid")?,
                    id.clone(),
                )?;
                let declared = find_operation(tasks, &key)?;
                if declared.from() != &page
                    || declared.to().is_some()
                    || declared.action() != &action
                {
                    return Err(AdmissionError::new("admission_navigation_action_mismatch"));
                }
                if !declared.destructive() {
                    return Err(AdmissionError::with_detail(
                        "admission_destructive_capability_invalid",
                        format!("destructive region operation '{key}' is not typed destructive"),
                    ));
                }
                Some(key)
            }
            (None, None) => None,
            _ => return Err(AdmissionError::new("admission_identity_conflict")),
        };
        destructive_regions.push(AdmittedDestructiveRegion {
            page,
            rect,
            operation,
        });
    }
    destructive_regions.sort_by(|left, right| {
        left.page
            .cmp(&right.page)
            .then_with(|| left.rect.x.cmp(&right.rect.x))
            .then_with(|| left.rect.y.cmp(&right.rect.y))
            .then_with(|| left.rect.width.cmp(&right.rect.width))
            .then_with(|| left.rect.height.cmp(&right.rect.height))
    });

    let mut control_points = Vec::with_capacity(raw.control_points.len());
    let mut control_point_names = BTreeSet::new();
    for point in &raw.control_points {
        let name = non_empty(
            &point.name,
            "admission_identity_invalid",
            "control point name",
        )?;
        if !control_point_names.insert(name.clone()) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        if point
            .note
            .as_deref()
            .is_some_and(|note| note.trim().is_empty() || note.trim() != note)
        {
            return Err(AdmissionError::new("admission_identity_invalid"));
        }
        let has_legacy_point = point.point.is_some() || point.x.is_some() || point.y.is_some();
        let action = match (&point.click, has_legacy_point) {
            (Some(click), false) => close_navigation_action(click, control, targets)?,
            (None, true) => {
                let (x, y) = close_point_components(point.point, point.x, point.y)?;
                let rect = close_click_rect(
                    RawRect {
                        x,
                        y,
                        width: 1,
                        height: 1,
                    },
                    control,
                )?;
                AdmittedAction::Tap {
                    point: rect.center(),
                    rect,
                }
            }
            (Some(_), true) | (None, false) => {
                return Err(AdmissionError::new("admission_operation_invalid"));
            }
        };
        if action.static_rects().is_empty() {
            return Err(AdmissionError::with_detail(
                "admission_control_point_capability_unknown",
                format!("control point '{name}' must have statically bounded effect coordinates"),
            ));
        }
        control_points.push(AdmittedControlPoint {
            name,
            action,
            effect_capability: AdmittedEffectCapability::NavigationOnly,
        });
    }
    control_points.sort_by(|left, right| left.name.cmp(&right.name));

    let effect_operations = routes
        .iter()
        .map(|route| route.operation().clone())
        .chain(page_operations.iter().cloned())
        .collect::<BTreeSet<_>>();
    for key in effect_operations {
        let operation = find_operation(tasks, &key)?;
        validate_non_destructive_effect(
            &format!("operation '{key}'"),
            operation.from(),
            operation.action(),
            operation.effect_capability(),
            &destructive_regions,
        )?;
    }
    for point in &control_points {
        validate_non_destructive_effect(
            &format!("control point '{}'", point.name()),
            &PageSelector::Any,
            point.action(),
            point.effect_capability(),
            &destructive_regions,
        )?;
    }

    Ok(Some(AdmittedNavigation {
        schema_version: raw.schema_version.clone(),
        routes,
        page_operations,
        destructive_regions,
        control_points,
    }))
}

fn validate_non_destructive_effect(
    label: &str,
    page: &PageSelector,
    action: &AdmittedAction,
    capability: AdmittedEffectCapability,
    destructive_regions: &[AdmittedDestructiveRegion],
) -> AdmissionResult<()> {
    if capability.requires_explicit_opt_in() {
        return Ok(());
    }
    for rect in action.static_rects() {
        if destructive_regions.iter().any(|destructive| {
            selectors_overlap(page, destructive.page()) && rect.intersects(destructive.rect())
        }) {
            return Err(AdmissionError::with_detail(
                "admission_destructive_overlap",
                format!("{label} overlaps a destructive region"),
            ));
        }
    }
    Ok(())
}

fn all_operations(
    tasks: &BTreeMap<TaskKey, AdmittedTask>,
) -> impl Iterator<Item = &AdmittedOperation> {
    tasks.values().flat_map(|task| task.operations())
}

fn find_operation<'a>(
    tasks: &'a BTreeMap<TaskKey, AdmittedTask>,
    key: &OperationKey,
) -> AdmissionResult<&'a AdmittedOperation> {
    tasks
        .get(key.task())
        .and_then(|task| task.operation(key))
        .ok_or_else(|| {
            AdmissionError::with_detail(
                "admission_missing_reference",
                format!("operation '{key}' is not packaged"),
            )
        })
}

fn close_page_selector(
    game: &str,
    raw: &str,
    pages: &BTreeMap<PageKey, AdmittedPage>,
) -> AdmissionResult<PageSelector> {
    let selector = if raw == "any" {
        PageSelector::Any
    } else {
        PageSelector::Exact(resolve_packaged_page(game, raw, pages)?)
    };
    if let PageSelector::Exact(page) = &selector
        && !pages.contains_key(page)
    {
        return Err(AdmissionError::with_detail(
            "admission_missing_reference",
            format!("page '{page}' is not packaged"),
        ));
    }
    Ok(selector)
}

fn selectors_overlap(left: &PageSelector, right: &PageSelector) -> bool {
    match (left, right) {
        (PageSelector::Any, _) | (_, PageSelector::Any) => true,
        (PageSelector::Exact(left), PageSelector::Exact(right)) => left == right,
    }
}

fn close_navigation_action(
    raw: &RawNavigationAction,
    control: &AdmittedControl,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<AdmittedAction> {
    match raw {
        RawNavigationAction::Point { point, x, y } => {
            let (x, y) = close_point_components(*point, *x, *y)?;
            let rect = close_click_rect(
                RawRect {
                    x,
                    y,
                    width: 1,
                    height: 1,
                },
                control,
            )?;
            Ok(AdmittedAction::Tap {
                point: rect.center(),
                rect,
            })
        }
        RawNavigationAction::Rect {
            x,
            y,
            width,
            height,
        } => {
            let rect = close_click_rect(
                RawRect {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                },
                control,
            )?;
            Ok(AdmittedAction::Tap {
                point: rect.center(),
                rect,
            })
        }
        RawNavigationAction::Target { target_id } => {
            close_navigation_target(target_id, TargetTapMode::Deterministic, targets)
        }
        RawNavigationAction::TargetCenter { target_id } => {
            close_navigation_target(target_id, TargetTapMode::Center, targets)
        }
        RawNavigationAction::Drag {
            from_rect,
            to_rect,
            duration_ms,
        } => {
            let from_rect = navigation_tap_rect(*from_rect, control)?;
            let to_rect = navigation_tap_rect(*to_rect, control)?;
            Ok(AdmittedAction::Drag {
                from: from_rect.center(),
                to: to_rect.center(),
                from_rect,
                to_rect,
                duration: InputDuration::new(*duration_ms)?,
            })
        }
    }
}

fn close_navigation_target(
    target_id: &str,
    mode: TargetTapMode,
    targets: &BTreeMap<TargetKey, AdmittedTarget>,
) -> AdmissionResult<AdmittedAction> {
    let target = TargetKey::parse(target_id.to_string(), "admission_target_closure")?;
    if targets.get(&target).map(AdmittedTarget::kind) != Some(AdmittedTargetKind::Template) {
        return Err(AdmissionError::new("admission_target_closure"));
    }
    Ok(AdmittedAction::TargetTap {
        target,
        mode,
        offset: None,
    })
}

fn close_navigation_tap(
    raw: &RawNavigationTapAction,
    control: &AdmittedControl,
) -> AdmissionResult<AdmittedAction> {
    let rect = navigation_tap_rect(*raw, control)?;
    Ok(AdmittedAction::Tap {
        point: rect.center(),
        rect,
    })
}

fn navigation_tap_rect(
    raw: RawNavigationTapAction,
    control: &AdmittedControl,
) -> AdmissionResult<BoundedRect> {
    match raw {
        RawNavigationTapAction::Point { point, x, y } => {
            let (x, y) = close_point_components(point, x, y)?;
            close_click_rect(
                RawRect {
                    x,
                    y,
                    width: 1,
                    height: 1,
                },
                control,
            )
        }
        RawNavigationTapAction::Rect {
            x,
            y,
            width,
            height,
        } => close_click_rect(
            RawRect {
                x,
                y,
                width,
                height,
            },
            control,
        ),
    }
}

fn close_point_components(
    point: Option<RawPointValue>,
    x: Option<i32>,
    y: Option<i32>,
) -> AdmissionResult<(i32, i32)> {
    match (point, x, y) {
        (Some(RawPointValue::Pair([x, y])), None, None) => Ok((x, y)),
        (Some(RawPointValue::Text(point)), None, None) => Ok((point.x, point.y)),
        (None, Some(x), Some(y)) => Ok((x, y)),
        _ => Err(AdmissionError::new("admission_operation_invalid")),
    }
}

fn validate_closed_manifest(raw: &RawManifestV03) -> AdmissionResult<()> {
    let mut declared = BTreeMap::<String, String>::new();
    for (path, hash) in &raw.hashes {
        validate_relative_ref(path).map_err(|_| AdmissionError::new("admission_asset_closure"))?;
        Sha256Hash::parse_hex(hash).map_err(|_| AdmissionError::new("admission_asset_closure"))?;
        if declared.insert(path.clone(), hash.clone()).is_some() {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
    }
    for file in &raw.files {
        validate_relative_ref(&file.path)
            .map_err(|_| AdmissionError::new("admission_asset_closure"))?;
        let hash = match (&file.sha256, &file.hash) {
            (Some(_), Some(_)) => {
                return Err(AdmissionError::new("admission_identity_conflict"));
            }
            (Some(hash), None) | (None, Some(hash)) => Some(hash),
            (None, None) => None,
        };
        if declared.contains_key(&file.path) {
            return Err(AdmissionError::new("admission_identity_conflict"));
        }
        if let Some(hash) = hash {
            Sha256Hash::parse_hex(hash)
                .map_err(|_| AdmissionError::new("admission_asset_closure"))?;
            declared.insert(file.path.clone(), hash.clone());
        }
    }
    Ok(())
}

pub(super) fn parse_package(
    entries: &BTreeMap<String, Vec<u8>>,
    metadata: &PackageMetadata,
) -> ContainmentResult<Option<ParsedPackage>> {
    if metadata.layout != PackageLayout::Lab {
        return Ok(None);
    }

    let control = adapt_control_v1(strict_entry(entries, "control.json", "control")?)?;

    let manifest = adapt_manifest_v03(
        &metadata.manifest_path,
        strict_entry(entries, &metadata.manifest_path, "manifest")?,
    )?;

    let task_prefix = prefixed_path(&metadata.resource_root, "operations/");
    let task_suffix = "/task.json";
    let mut tasks = BTreeMap::new();
    for path in entries
        .keys()
        .filter(|path| path.starts_with(&task_prefix) && path.ends_with(task_suffix))
    {
        let path_key = &path[task_prefix.len()..path.len() - task_suffix.len()];
        if path_key.is_empty() || path_key.contains('/') {
            return Err(package_contract_error(
                path,
                "operation task path must contain exactly one non-empty task id",
            ));
        }
        let task = adapt_operation_document(path, strict_entry(entries, path, "operation task")?)?;
        if task.task_id != path_key {
            return Err(package_contract_error(
                path,
                format!(
                    "task_id '{}' does not match operation path task id '{path_key}'",
                    task.task_id
                ),
            ));
        }
        if tasks.insert(path_key.to_string(), task).is_some() {
            return Err(package_contract_error(
                path,
                format!("operation task id '{path_key}' is duplicated"),
            ));
        }
    }

    let recognition_path = metadata.recognition_pack_path.as_deref().ok_or_else(|| {
        package_contract_error(
            "control.json",
            "executable Lab package is missing its recognition pack",
        )
    })?;
    let recognition = adapt_recognition_document(
        recognition_path,
        strict_entry(entries, recognition_path, "recognition pack")?,
    )?;

    let pages_path = metadata.pages_path.as_deref().ok_or_else(|| {
        package_contract_error(
            "control.json",
            "executable Lab package is missing its page definitions",
        )
    })?;
    let pages = adapt_page_document(pages_path, strict_entry(entries, pages_path, "page set")?)?;

    let navigation = metadata
        .navigation_path
        .as_deref()
        .map(|path| {
            adapt_navigation_document(path, strict_entry(entries, path, "navigation contract")?)
        })
        .transpose()?;

    Ok(Some(ParsedPackage {
        control,
        manifest,
        tasks,
        recognition,
        pages,
        navigation,
    }))
}

fn strict_entry<T: for<'de> Deserialize<'de>>(
    entries: &BTreeMap<String, Vec<u8>>,
    path: &str,
    label: &str,
) -> ContainmentResult<T> {
    read_json_entry(entries, path).map_err(|error| match error {
        ContainmentError::JsonParse { message, .. } => {
            package_contract_error(path, format!("strict {label} parsing failed: {message}"))
        }
        other => other,
    })
}

fn require_schema(path: &str, actual: &str, supported: &[&str]) -> ContainmentResult<()> {
    if supported.contains(&actual) {
        return Ok(());
    }
    Err(package_contract_error(
        path,
        format!(
            "unsupported schema_version '{actual}'; expected one of {}",
            supported.join(", ")
        ),
    ))
}

// Each accepted wire version has an explicit adapter entry point. The currently supported
// versions are structurally identical, so the adapters intentionally preserve the parsed DTO.
// Keeping the match here prevents a future schema from silently inheriting older semantics.
fn adapt_control_v1(raw: RawControlV1) -> ContainmentResult<RawControlV1> {
    require_schema("control.json", &raw.schema_version, &[CONTROL_SCHEMA_V1])?;
    Ok(raw)
}

fn adapt_manifest_v03(path: &str, raw: RawManifestV03) -> ContainmentResult<RawManifestV03> {
    require_schema(path, &raw.schema_version, &[MANIFEST_SCHEMA_V03])?;
    Ok(raw)
}

fn adapt_operation_document(
    path: &str,
    raw: RawTaskDocument,
) -> ContainmentResult<RawTaskDocument> {
    match raw.schema_version.as_str() {
        "0.3" => adapt_operation_v03(raw),
        "0.4" => adapt_operation_v04(raw),
        "0.5" => adapt_operation_v05(raw),
        "0.6" => adapt_operation_v06(raw),
        _ => {
            require_schema(path, &raw.schema_version, OPERATION_SCHEMAS)?;
            Ok(raw)
        }
    }
}

fn adapt_operation_v03(raw: RawTaskDocument) -> ContainmentResult<RawTaskDocument> {
    Ok(raw)
}

fn adapt_operation_v04(raw: RawTaskDocument) -> ContainmentResult<RawTaskDocument> {
    Ok(raw)
}

fn adapt_operation_v05(raw: RawTaskDocument) -> ContainmentResult<RawTaskDocument> {
    Ok(raw)
}

fn adapt_operation_v06(raw: RawTaskDocument) -> ContainmentResult<RawTaskDocument> {
    Ok(raw)
}

fn adapt_recognition_document(
    path: &str,
    raw: RawRecognitionDocument,
) -> ContainmentResult<RawRecognitionDocument> {
    match raw.schema_version.as_str() {
        "0.1" => adapt_recognition_v01(raw),
        "0.3" => adapt_recognition_v03(raw),
        "0.4" => adapt_recognition_v04(raw),
        "0.5" => adapt_recognition_v05(raw),
        _ => {
            require_schema(path, &raw.schema_version, RECOGNITION_SCHEMAS)?;
            Ok(raw)
        }
    }
}

fn adapt_recognition_v01(raw: RawRecognitionDocument) -> ContainmentResult<RawRecognitionDocument> {
    Ok(raw)
}

fn adapt_recognition_v03(raw: RawRecognitionDocument) -> ContainmentResult<RawRecognitionDocument> {
    Ok(raw)
}

fn adapt_recognition_v04(raw: RawRecognitionDocument) -> ContainmentResult<RawRecognitionDocument> {
    Ok(raw)
}

fn adapt_recognition_v05(raw: RawRecognitionDocument) -> ContainmentResult<RawRecognitionDocument> {
    Ok(raw)
}

fn adapt_page_document(path: &str, raw: RawPageDocument) -> ContainmentResult<RawPageDocument> {
    match raw.schema_version.as_str() {
        "0.1" => adapt_pages_v01(raw),
        "0.3" => adapt_pages_v03(raw),
        "0.4" => adapt_pages_v04(raw),
        "0.5" => adapt_pages_v05(raw),
        _ => {
            require_schema(path, &raw.schema_version, PAGE_SCHEMAS)?;
            Ok(raw)
        }
    }
}

fn adapt_pages_v01(raw: RawPageDocument) -> ContainmentResult<RawPageDocument> {
    Ok(raw)
}

fn adapt_pages_v03(raw: RawPageDocument) -> ContainmentResult<RawPageDocument> {
    Ok(raw)
}

fn adapt_pages_v04(raw: RawPageDocument) -> ContainmentResult<RawPageDocument> {
    Ok(raw)
}

fn adapt_pages_v05(raw: RawPageDocument) -> ContainmentResult<RawPageDocument> {
    Ok(raw)
}

fn adapt_navigation_document(
    path: &str,
    raw: RawNavigationDocument,
) -> ContainmentResult<RawNavigationDocument> {
    match raw.schema_version.as_str() {
        "0.3" => adapt_navigation_v03(raw),
        "0.4" => adapt_navigation_v04(raw),
        "0.5" => adapt_navigation_v05(raw),
        _ => {
            require_schema(path, &raw.schema_version, NAVIGATION_SCHEMAS)?;
            Ok(raw)
        }
    }
}

fn adapt_navigation_v03(raw: RawNavigationDocument) -> ContainmentResult<RawNavigationDocument> {
    Ok(raw)
}

fn adapt_navigation_v04(raw: RawNavigationDocument) -> ContainmentResult<RawNavigationDocument> {
    Ok(raw)
}

fn adapt_navigation_v05(raw: RawNavigationDocument) -> ContainmentResult<RawNavigationDocument> {
    Ok(raw)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawControlV1 {
    pub(super) schema_version: String,
    pub(super) package_id: String,
    pub(super) execution_mode: String,
    pub(super) game: String,
    pub(super) server: String,
    pub(super) resolution: RawResolution,
    pub(super) entry_task_id: String,
    #[serde(default)]
    pub(super) resource_root: Option<String>,
    #[serde(default)]
    pub(super) capture_interval_ms: Option<u64>,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) step_timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) max_steps: Option<u32>,
    #[serde(default)]
    pub(super) stop_on_error: Option<bool>,
    #[serde(default)]
    pub(super) stop_on_confirmation: Option<bool>,
    #[serde(default)]
    pub(super) allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    pub(super) output: Option<Value>,
    #[serde(default)]
    pub(super) capture_backend: Option<String>,
    #[serde(default)]
    pub(super) frame_store: RawFrameStoreControl,
    #[serde(default)]
    pub(super) producer: Option<Value>,
    #[serde(default)]
    pub(super) trusted_execution: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawResolution {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFrameStoreControl {
    #[serde(default)]
    pub(super) similarity_threshold: Option<f32>,
    #[serde(default)]
    pub(super) tier1_ratio: Option<f64>,
    #[serde(default)]
    pub(super) tier2_ratio: Option<f64>,
    #[serde(default)]
    pub(super) tier3_ratio: Option<f64>,
    #[serde(default)]
    pub(super) hysteresis_ratio: Option<f64>,
    #[serde(default)]
    pub(super) max_mem_bytes: Option<u64>,
    #[serde(default)]
    pub(super) os_reserve_bytes: Option<u64>,
    #[serde(default)]
    pub(super) flush_workspace_reserve_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifestV03 {
    pub(super) schema_version: String,
    pub(super) entry_task_id: String,
    #[serde(default)]
    pub(super) hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) files: Vec<RawManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifestFile {
    pub(super) path: String,
    #[serde(default)]
    pub(super) sha256: Option<String>,
    #[serde(default)]
    pub(super) hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTaskDocument {
    pub(super) schema_version: String,
    pub(super) task_id: String,
    pub(super) game: String,
    #[serde(default)]
    pub(super) server_scope: Vec<String>,
    #[serde(default)]
    pub(super) locale: Option<String>,
    #[serde(default)]
    pub(super) goal: String,
    pub(super) coordinate_space: RawResolution,
    #[serde(default)]
    pub(super) defaults: RawOperationDefaults,
    #[serde(default)]
    pub(super) anchors: Vec<RawOperationAnchor>,
    #[serde(default)]
    pub(super) color_probes: Vec<RawTaskColorProbe>,
    #[serde(default)]
    pub(super) verify_templates: Vec<RawTaskVerifyTemplate>,
    #[serde(default)]
    pub(super) entry_page: Option<String>,
    #[serde(default)]
    pub(super) target_page: Option<String>,
    #[serde(default)]
    pub(super) error_pages: Vec<String>,
    #[serde(default)]
    pub(super) recovery: Option<RawTaskRecovery>,
    #[serde(default)]
    pub(super) max_task_retries: Option<u32>,
    #[serde(default)]
    pub(super) on_exhausted: Option<String>,
    #[serde(default)]
    pub(super) page_rules: BTreeMap<String, RawPageRule>,
    pub(super) operations: Vec<RawOperation>,
    #[serde(default)]
    pub(super) provenance: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPageRule {
    #[serde(default)]
    pub(super) required: Vec<String>,
    #[serde(default)]
    pub(super) any_of: Vec<Vec<String>>,
    #[serde(default)]
    pub(super) optional: Vec<String>,
    #[serde(default)]
    pub(super) forbidden: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum RawTaskRecovery {
    Kind(String),
    Config(RawTaskRecoveryConfig),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTaskRecoveryConfig {
    pub(super) kind: String,
    #[serde(default)]
    pub(super) task_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOperationDefaults {
    #[serde(default)]
    pub(super) template_threshold: Option<f32>,
    #[serde(default)]
    pub(super) color_max_distance: Option<f32>,
    #[serde(default)]
    pub(super) match_metric: Option<RawMatchMetric>,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) max_attempts: Option<u32>,
    #[serde(default)]
    pub(super) retry_interval_ms: Option<u64>,
    #[serde(default)]
    pub(super) pre_delay_ms: Option<u64>,
    #[serde(default)]
    pub(super) post_delay_ms: Option<u64>,
    #[serde(default)]
    pub(super) pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    pub(super) post_wait_freezes_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOperationAnchor {
    pub(super) id: String,
    pub(super) template: String,
    #[serde(default)]
    pub(super) region: Option<RawTaskRegion>,
    #[serde(default)]
    pub(super) threshold: Option<f32>,
    #[serde(default)]
    pub(super) color_check: Option<Value>,
    #[serde(default)]
    pub(super) provenance: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawTaskRegion {
    Auto,
    FullFrame,
    Rect { rect: RawRect },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTaskColorProbe {
    pub(super) id: String,
    pub(super) region: RawTaskRegion,
    pub(super) expected: [u8; 3],
    #[serde(default)]
    pub(super) provenance: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTaskVerifyTemplate {
    pub(super) id: String,
    pub(super) template: String,
    pub(super) region: RawTaskRegion,
    #[serde(default)]
    pub(super) threshold: Option<f32>,
    #[serde(default)]
    pub(super) provenance: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOperation {
    pub(super) id: String,
    #[serde(default)]
    pub(super) purpose: String,
    pub(super) from: String,
    #[serde(default)]
    pub(super) to: Option<String>,
    pub(super) click: RawAction,
    #[serde(default)]
    pub(super) verify_template: Option<String>,
    #[serde(default)]
    pub(super) expect_after: Option<RawOperationExpectation>,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) max_attempts: Option<u32>,
    #[serde(default)]
    pub(super) retry_interval_ms: Option<u64>,
    #[serde(default)]
    pub(super) pre_delay_ms: Option<u64>,
    #[serde(default)]
    pub(super) post_delay_ms: Option<u64>,
    #[serde(default)]
    pub(super) pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    pub(super) post_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    pub(super) retryable: Option<bool>,
    #[serde(default)]
    pub(super) effect: Option<String>,
    #[serde(default)]
    pub(super) destructive: bool,
    #[serde(default)]
    pub(super) on_error: Option<String>,
    #[serde(default)]
    pub(super) guard: Option<RawOperationGuard>,
    #[serde(default)]
    pub(super) unguarded_trusted_coordinate: bool,
    #[serde(default)]
    pub(super) consumes: Vec<String>,
    #[serde(default)]
    pub(super) produces: Vec<String>,
    #[serde(default)]
    pub(super) verified_live: Option<bool>,
    #[serde(default)]
    pub(super) provenance: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOperationExpectation {
    pub(super) page_id: String,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawOperationGuard {
    pub(super) page_id: String,
    pub(super) target_id: String,
    pub(super) expected_rect: RawRect,
    #[serde(default)]
    pub(super) verify_template: Option<String>,
    #[serde(default)]
    pub(super) color_probe: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawAction {
    Point {
        x: i32,
        y: i32,
    },
    Rect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    SpecificRect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    LongPress {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Target {
        #[serde(default)]
        target_id: Option<String>,
        #[serde(default)]
        offset: Option<RawRect>,
    },
    TargetCenter {
        #[serde(default)]
        target_id: Option<String>,
        #[serde(default)]
        offset: Option<RawRect>,
    },
    Offset {
        #[serde(default)]
        target_id: Option<String>,
        offset: RawRect,
    },
    Drag {
        #[serde(rename = "from")]
        from_rect: RawRect,
        #[serde(rename = "to")]
        to_rect: RawRect,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRecognitionDocument {
    pub(super) schema_version: String,
    #[serde(default)]
    pub(super) converter_schema_version: Option<String>,
    #[serde(default)]
    pub(super) generated: Option<bool>,
    #[serde(default)]
    pub(super) generated_by: Option<String>,
    #[serde(default)]
    pub(super) game: Option<String>,
    #[serde(default)]
    pub(super) server: Option<String>,
    #[serde(default)]
    pub(super) locale: Option<String>,
    #[serde(default)]
    pub(super) coordinate_space: Option<RawResolution>,
    #[serde(default)]
    pub(super) defaults: RawRecognitionDefaults,
    pub(super) targets: Vec<RawRecognitionTarget>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRecognitionDefaults {
    #[serde(default)]
    pub(super) template_threshold: Option<f32>,
    #[serde(default)]
    pub(super) color_max_distance: Option<f32>,
    #[serde(default)]
    pub(super) match_metric: Option<RawMatchMetric>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawMatchMetric {
    CcorrNormed,
    CcoeffNormed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawRecognitionTarget {
    Template {
        id: String,
        template_path: String,
        region: RawRegion,
        #[serde(default)]
        threshold: Option<f32>,
        #[serde(default)]
        method: Option<RawRecognitionMethod>,
        #[serde(default)]
        mask: Option<RawRecognitionMask>,
        #[serde(default)]
        rect_move: Option<RawRect>,
        #[serde(default)]
        color_check: Option<RawColorCheck>,
        #[serde(default)]
        click: Option<RawRect>,
    },
    Color {
        id: String,
        region: RawRect,
        expected: [u8; 3],
        #[serde(default)]
        click: Option<RawRect>,
    },
    ClickOnly {
        id: String,
        click: RawRect,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawRecognitionMethod {
    Ncc,
    RgbCount,
    HsvCount,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum RawRegion {
    Rect(RawRect),
    Keyword(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawRecognitionMask {
    Range { lower: u8, upper: u8 },
    Bitmap { path: String },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawColorCheck {
    pub(super) region: RawRect,
    pub(super) expected: [u8; 3],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPageDocument {
    pub(super) schema_version: String,
    #[serde(default)]
    pub(super) converter_schema_version: Option<String>,
    #[serde(default)]
    pub(super) generated: Option<bool>,
    #[serde(default)]
    pub(super) generated_by: Option<String>,
    pub(super) pages: Vec<RawPageDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPageDefinition {
    pub(super) id: String,
    pub(super) required: Vec<String>,
    #[serde(default)]
    pub(super) any_of: Vec<Vec<String>>,
    #[serde(default)]
    pub(super) optional: Vec<String>,
    #[serde(default)]
    pub(super) forbidden: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNavigationDocument {
    pub(super) schema_version: String,
    #[serde(default)]
    pub(super) converter_schema_version: Option<String>,
    #[serde(default)]
    pub(super) generated: Option<bool>,
    #[serde(default)]
    pub(super) generated_by: Option<String>,
    pub(super) game: String,
    pub(super) server: String,
    #[serde(default)]
    pub(super) coordinate_space: Option<RawResolution>,
    #[serde(rename = "navigation")]
    pub(super) routes: Vec<RawNavigationRoute>,
    #[serde(default)]
    pub(super) page_operations: Vec<RawNavigationPageAction>,
    #[serde(default)]
    pub(super) destructive_actions: Vec<RawNavigationDestructiveAction>,
    #[serde(default)]
    pub(super) control_points: Vec<RawNavigationControlPoint>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNavigationRoute {
    pub(super) id: String,
    pub(super) from_page: String,
    pub(super) to_page: String,
    pub(super) click: RawNavigationAction,
    #[serde(default)]
    pub(super) source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNavigationPageAction {
    pub(super) task_id: String,
    pub(super) id: String,
    pub(super) page: String,
    pub(super) click: RawNavigationAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNavigationDestructiveAction {
    #[serde(default)]
    pub(super) task_id: Option<String>,
    #[serde(default)]
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) page: Option<String>,
    pub(super) click: RawNavigationTapAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawNavigationControlPoint {
    pub(super) name: String,
    #[serde(default)]
    pub(super) click: Option<RawNavigationAction>,
    #[serde(default)]
    pub(super) point: Option<RawPointValue>,
    #[serde(default)]
    pub(super) x: Option<i32>,
    #[serde(default)]
    pub(super) y: Option<i32>,
    #[serde(default)]
    pub(super) note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawNavigationAction {
    Point {
        #[serde(default)]
        point: Option<RawPointValue>,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
    Rect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Target {
        target_id: String,
    },
    TargetCenter {
        target_id: String,
    },
    Drag {
        #[serde(rename = "from")]
        from_rect: RawNavigationTapAction,
        #[serde(rename = "to")]
        to_rect: RawNavigationTapAction,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RawNavigationTapAction {
    Point {
        #[serde(default)]
        point: Option<RawPointValue>,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
    Rect {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub(super) enum RawPointValue {
    Pair([i32; 2]),
    Text(RawPointText),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RawPointText {
    pub(super) x: i32,
    pub(super) y: i32,
}

impl<'de> Deserialize<'de> for RawPointText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let mut parts = value.split(',').map(str::trim);
        let x = parts
            .next()
            .ok_or_else(|| serde::de::Error::custom("point text is missing x"))?
            .parse::<i32>()
            .map_err(serde::de::Error::custom)?;
        let y = parts
            .next()
            .ok_or_else(|| serde::de::Error::custom("point text is missing y"))?
            .parse::<i32>()
            .map_err(serde::de::Error::custom)?;
        if parts.next().is_some() {
            return Err(serde::de::Error::custom(
                "point text must contain exactly x,y",
            ));
        }
        Ok(Self { x, y })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_package_fixture() -> (BTreeMap<String, Vec<u8>>, PackageMetadata) {
        let entries = BTreeMap::from([
            (
                "control.json".to_string(),
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"neutral.fixture",
                    "execution_mode":"recognize_only",
                    "game":"neutral",
                    "server":"test",
                    "resolution":{"width":2,"height":1},
                    "entry_task_id":"task"
                }"#
                .to_vec(),
            ),
            (
                "resources/manifest.json".to_string(),
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#.to_vec(),
            ),
            (
                "resources/operations/task/task.json".to_string(),
                br#"{
                    "schema_version":"0.6",
                    "task_id":"task",
                    "game":"neutral",
                    "server_scope":["test"],
                    "coordinate_space":{"width":2,"height":1},
                    "operations":[{
                        "id":"tap",
                        "from":"home",
                        "click":{"kind":"point","x":1,"y":0},
                        "unguarded_trusted_coordinate":true
                    }]
                }"#
                .to_vec(),
            ),
            (
                "resources/recognition/neutral.test.pack.json".to_string(),
                br#"{
                    "schema_version":"0.5",
                    "game":"neutral",
                    "server":"test",
                    "coordinate_space":{"width":2,"height":1},
                    "targets":[{
                        "type":"color",
                        "id":"page/home",
                        "region":{"x":0,"y":0,"width":1,"height":1},
                        "expected":[0,0,0]
                    }]
                }"#
                .to_vec(),
            ),
            (
                "resources/recognition/neutral.test.pages.json".to_string(),
                br#"{
                    "schema_version":"0.5",
                    "pages":[{"id":"neutral/home","required":["page/home"]}]
                }"#
                .to_vec(),
            ),
            (
                "resources/navigation/neutral.test.navigation.json".to_string(),
                br#"{
                    "schema_version":"0.5",
                    "game":"neutral",
                    "server":"test",
                    "navigation":[],
                    "destructive_actions":[]
                }"#
                .to_vec(),
            ),
        ]);
        let metadata = PackageMetadata {
            layout: PackageLayout::Lab,
            task_id: super::super::TaskId::new("task").expect("task id"),
            resource_root: "resources".to_string(),
            manifest_path: "resources/manifest.json".to_string(),
            manifest: serde_json::json!({}),
            operation_path: "resources/operations/task/task.json".to_string(),
            recognition_pack_path: Some("resources/recognition/neutral.test.pack.json".to_string()),
            pages_path: Some("resources/recognition/neutral.test.pages.json".to_string()),
            navigation_path: Some("resources/navigation/neutral.test.navigation.json".to_string()),
        };
        (entries, metadata)
    }

    fn mutate_json(
        entries: &mut BTreeMap<String, Vec<u8>>,
        path: &str,
        mutation: impl FnOnce(&mut Value),
    ) {
        let mut value: Value = serde_json::from_slice(&entries[path]).expect("fixture JSON value");
        mutation(&mut value);
        entries.insert(
            path.to_string(),
            serde_json::to_vec(&value).expect("fixture JSON"),
        );
    }

    fn close_fixture(
        entries: &BTreeMap<String, Vec<u8>>,
        metadata: &PackageMetadata,
    ) -> AdmissionResult<ClosedPackage> {
        let parsed = parse_package(entries, metadata)
            .expect("strict fixture parsing")
            .expect("Lab parsed package");
        close_package(parsed, entries, &metadata.resource_root)
    }

    fn assert_close_code(
        entries: &BTreeMap<String, Vec<u8>>,
        metadata: &PackageMetadata,
        code: &'static str,
    ) {
        let error = close_fixture(entries, metadata).expect_err("package closure must fail");
        assert_eq!(error.code(), code, "unexpected admission error: {error}");
    }

    #[test]
    fn parsed_package_uses_explicit_supported_version_adapters() {
        let (entries, metadata) = parsed_package_fixture();

        let parsed = parse_package(&entries, &metadata)
            .expect("strict package parsing")
            .expect("Lab parsed package");

        assert_eq!(parsed.control.schema_version, CONTROL_SCHEMA_V1);
        assert_eq!(parsed.manifest.schema_version, MANIFEST_SCHEMA_V03);
        assert_eq!(parsed.tasks["task"].schema_version, "0.6");
        assert_eq!(parsed.recognition.schema_version, "0.5");
        assert_eq!(parsed.pages.schema_version, "0.5");
        assert_eq!(
            parsed
                .navigation
                .as_ref()
                .map(|value| value.schema_version.as_str()),
            Some("0.5")
        );
    }

    #[test]
    fn closed_package_constructs_canonical_keys_and_bounded_action() {
        let (entries, metadata) = parsed_package_fixture();
        let parsed = parse_package(&entries, &metadata)
            .expect("strict package parsing")
            .expect("Lab parsed package");

        let closed = close_package(parsed, &entries, &metadata.resource_root)
            .expect("canonical package closure");

        assert_eq!(closed.control.entry_task().as_str(), "task");
        assert_eq!(closed.entry_task.key().as_str(), "task");
        assert_eq!(closed.entry_task.operations().len(), 1);
        assert_eq!(
            closed.entry_task.operations()[0].from(),
            &PageSelector::Exact(PageKey::parse("neutral", "neutral/home").expect("page key"))
        );
        assert!(matches!(
            closed.entry_task.operations()[0].action(),
            AdmittedAction::Tap { point, .. } if point.x() == 1 && point.y() == 0
        ));
    }

    #[test]
    fn closure_rejects_negative_overflow_and_excessive_input_duration() {
        for action in [
            serde_json::json!({"kind":"point","x":-1,"y":0}),
            serde_json::json!({
                "kind":"rect",
                "x": i32::MAX,
                "y": 0,
                "width": 2,
                "height": 1
            }),
        ] {
            let (mut entries, metadata) = parsed_package_fixture();
            mutate_json(
                &mut entries,
                "resources/operations/task/task.json",
                |task| task["operations"][0]["click"] = action,
            );
            assert_close_code(&entries, &metadata, "admission_input_bounds_invalid");
        }

        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| {
                task["operations"][0]["click"] = serde_json::json!({
                    "kind":"long_tap",
                    "x":1,
                    "y":0,
                    "duration_ms":60_001
                });
            },
        );
        assert_close_code(&entries, &metadata, "admission_input_duration_invalid");
    }

    #[test]
    fn closure_follows_recovery_and_on_error_to_fixed_point_and_rejects_cycles() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| {
                task["recovery"] = serde_json::json!({"kind":"return_home","task_id":"recovery"});
            },
        );
        let mut recovery: Value =
            serde_json::from_slice(&entries["resources/operations/task/task.json"])
                .expect("task JSON");
        recovery["task_id"] = Value::String("recovery".to_string());
        recovery["recovery"] =
            serde_json::json!({"kind":"return_home","task_id":"missing_transitive"});
        entries.insert(
            "resources/operations/recovery/task.json".to_string(),
            serde_json::to_vec(&recovery).expect("recovery JSON"),
        );
        assert_close_code(&entries, &metadata, "admission_missing_reference");

        recovery["recovery"] = serde_json::json!({"kind":"return_home","task_id":"task"});
        entries.insert(
            "resources/operations/recovery/task.json".to_string(),
            serde_json::to_vec(&recovery).expect("recovery JSON"),
        );
        assert_close_code(&entries, &metadata, "admission_recovery_cycle");

        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["on_error"] = Value::String("missing".to_string()),
        );
        assert_close_code(&entries, &metadata, "admission_missing_reference");
    }

    #[test]
    fn navigable_mode_requires_nonempty_navigation_and_exact_duplicate_semantics() {
        let (mut entries, mut metadata) = parsed_package_fixture();
        mutate_json(&mut entries, "control.json", |control| {
            control["execution_mode"] = Value::String("navigable_route".to_string());
        });
        entries.remove("resources/navigation/neutral.test.navigation.json");
        metadata.navigation_path = None;
        assert_close_code(&entries, &metadata, "admission_mode_requirements");

        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(&mut entries, "control.json", |control| {
            control["execution_mode"] = Value::String("navigable_route".to_string());
        });
        assert_close_code(&entries, &metadata, "admission_mode_requirements");

        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(&mut entries, "control.json", |control| {
            control["execution_mode"] = Value::String("navigable_route".to_string());
        });
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["to"] = Value::String("neutral/home".to_string()),
        );
        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| {
                navigation["navigation"] = serde_json::json!([{
                    "id":"tap",
                    "from_page":"neutral/home",
                    "to_page":"home",
                    "click":{"kind":"point","x":1,"y":0}
                }]);
            },
        );
        let closed = close_fixture(&entries, &metadata).expect("canonical route closure");
        assert_eq!(
            closed.navigation.as_ref().expect("navigation").routes()[0]
                .operation()
                .to_string(),
            "task::tap"
        );

        mutate_json(&mut entries, "control.json", |control| {
            control["allow_placeholder_coords"] = Value::Bool(true);
        });
        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| navigation["navigation"][0]["click"]["x"] = Value::from(0),
        );
        assert_close_code(&entries, &metadata, "admission_navigation_action_mismatch");
    }

    #[test]
    fn canonical_page_selector_blocks_spelling_based_destructive_overlap() {
        for page in ["home", "neutral/home"] {
            let (mut entries, metadata) = parsed_package_fixture();
            mutate_json(&mut entries, "control.json", |control| {
                control["execution_mode"] = Value::String("navigable_route".to_string());
            });
            mutate_json(
                &mut entries,
                "resources/operations/task/task.json",
                |task| {
                    task["operations"][0]["to"] = Value::String("home".to_string());
                },
            );
            mutate_json(
                &mut entries,
                "resources/navigation/neutral.test.navigation.json",
                |navigation| {
                    navigation["navigation"] = serde_json::json!([{
                        "id":"tap",
                        "from_page":"neutral/home",
                        "to_page":"home",
                        "click":{"kind":"point","x":1,"y":0}
                    }]);
                    navigation["destructive_actions"] = serde_json::json!([{
                        "page":page,
                        "click":{"kind":"point","x":1,"y":0}
                    }]);
                },
            );
            assert_close_code(&entries, &metadata, "admission_destructive_overlap");
        }
    }

    #[test]
    fn control_points_share_the_canonical_destructive_overlap_gate() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| {
                navigation["destructive_actions"] = serde_json::json!([{
                    "page":"any",
                    "click":{"kind":"point","x":1,"y":0}
                }]);
                navigation["control_points"] = serde_json::json!([{
                    "name":"wake",
                    "point":[1,0]
                }]);
            },
        );

        assert_close_code(&entries, &metadata, "admission_destructive_overlap");
    }

    #[test]
    fn destructive_navigation_requires_typed_operation_capability() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| {
                navigation["page_operations"] = serde_json::json!([{
                    "task_id":"task",
                    "id":"tap",
                    "page":"home",
                    "click":{"kind":"point","x":1,"y":0}
                }]);
                navigation["destructive_actions"] = serde_json::json!([{
                    "task_id":"task",
                    "id":"tap",
                    "page":"home",
                    "click":{"kind":"point","x":1,"y":0}
                }]);
            },
        );

        assert_close_code(
            &entries,
            &metadata,
            "admission_destructive_capability_invalid",
        );

        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["destructive"] = Value::Bool(true),
        );
        let closed = close_fixture(&entries, &metadata).expect("typed destructive operation");
        assert_eq!(
            closed.entry_task.operations()[0].effect_capability(),
            AdmittedEffectCapability::Destructive
        );
    }

    #[test]
    fn guarded_static_tap_is_a_typed_color_target_authority() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| {
                task["operations"][0]["guard"] = serde_json::json!({
                    "page_id":"neutral/home",
                    "target_id":"page/home",
                    "expected_rect":{"x":0,"y":0,"width":1,"height":1},
                    "color_probe":"page/home"
                });
                task["operations"][0]["unguarded_trusted_coordinate"] = Value::Bool(false);
            },
        );
        let admitted = admit_package(close_fixture(&entries, &metadata).expect("closed package"))
            .expect("admitted package");

        let operation = admitted
            .target_operation("page/home")
            .expect("typed color target authority");
        assert_eq!(operation.key().operation(), "tap");
        assert_eq!(
            operation.effect_capability(),
            AdmittedEffectCapability::NavigationOnly
        );
    }

    #[test]
    fn target_offset_requires_a_possible_domain_within_resolution() {
        let resolution = PackageResolution::new(10, 10).expect("resolution");

        assert_eq!(
            TargetOffset::new(
                RawRect {
                    x: 0,
                    y: 0,
                    width: 11,
                    height: 1,
                },
                resolution,
            )
            .expect_err("impossible target offset")
            .code(),
            "admission_input_bounds_invalid"
        );
        assert!(
            TargetOffset::new(
                RawRect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
                resolution,
            )
            .is_ok()
        );
    }

    #[test]
    fn admitted_target_offsets_have_a_total_static_projection_domain() {
        let resolution = PackageResolution::new(10, 10).expect("resolution");

        for x in -1..=10 {
            for y in -1..=10 {
                for width in -1..=11 {
                    for height in -1..=11 {
                        let result = TargetOffset::new(
                            RawRect {
                                x,
                                y,
                                width,
                                height,
                            },
                            resolution,
                        );
                        if let Ok(offset) = result {
                            let right = offset
                                .x()
                                .checked_add(offset.width() - 1)
                                .expect("admitted offset right edge");
                            let bottom = offset
                                .y()
                                .checked_add(offset.height() - 1)
                                .expect("admitted offset bottom edge");
                            assert!(offset.x() >= 0 && offset.y() >= 0);
                            assert!(offset.width() > 0 && offset.height() > 0);
                            assert!(right < resolution.width() as i32);
                            assert!(bottom < resolution.height() as i32);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn any_is_a_typed_selector_and_dangling_navigation_target_is_rejected() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(&mut entries, "control.json", |control| {
            control["execution_mode"] = Value::String("navigable_route".to_string());
        });
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| {
                task["operations"][0]["from"] = Value::String("any".to_string());
                task["operations"][0]["to"] = Value::String("home".to_string());
            },
        );
        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| {
                navigation["navigation"] = serde_json::json!([{
                    "id":"tap",
                    "from_page":"any",
                    "to_page":"neutral/home",
                    "click":{"kind":"point","x":1,"y":0}
                }]);
            },
        );
        let closed = close_fixture(&entries, &metadata).expect("typed any route");
        assert_eq!(closed.entry_task.operations()[0].from(), &PageSelector::Any);

        mutate_json(
            &mut entries,
            "resources/navigation/neutral.test.navigation.json",
            |navigation| {
                navigation["control_points"] = serde_json::json!([{
                    "name":"dangling",
                    "click":{"kind":"target_center","target_id":"missing"}
                }]);
            },
        );
        assert_close_code(&entries, &metadata, "admission_target_closure");
    }

    #[test]
    fn parsed_package_rejects_unknown_safety_fields_at_the_entry_boundary() {
        for path in [
            "control.json",
            "resources/manifest.json",
            "resources/operations/task/task.json",
            "resources/recognition/neutral.test.pack.json",
            "resources/recognition/neutral.test.pages.json",
            "resources/navigation/neutral.test.navigation.json",
        ] {
            let (mut entries, metadata) = parsed_package_fixture();
            let mut value: Value =
                serde_json::from_slice(&entries[path]).expect("fixture JSON value");
            value
                .as_object_mut()
                .expect("fixture JSON object")
                .insert("unsafe_future_field".to_string(), Value::Bool(true));
            entries.insert(
                path.to_string(),
                serde_json::to_vec(&value).expect("fixture JSON"),
            );

            let error = parse_package(&entries, &metadata)
                .expect_err("unknown safety field must fail at package parsing");
            assert!(
                matches!(error, ContainmentError::PackageContract { .. }),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[test]
    fn strict_action_rejects_unknown_and_conflicting_fields() {
        for source in [
            r#"{"kind":"point","x":1,"y":2,"typo":3}"#,
            r#"{"kind":"point","x":1,"y":2,"width":3}"#,
            r#"{"kind":"drag","from":{"kind":"rect","x":0,"y":0,"width":1,"height":1},"to":{"x":1,"y":1,"width":1,"height":1},"duration_ms":1}"#,
        ] {
            assert!(
                serde_json::from_str::<RawAction>(source).is_err(),
                "strict operation action unexpectedly accepted {source}"
            );
        }
    }

    #[test]
    fn strict_navigation_action_requires_duration_and_rejects_unknown_fields() {
        for source in [
            r#"{"kind":"drag","from":{"kind":"point","x":0,"y":0},"to":{"kind":"point","x":1,"y":1}}"#,
            r#"{"kind":"point","x":1,"y":2,"duration_ms":3}"#,
        ] {
            assert!(
                serde_json::from_str::<RawNavigationAction>(source).is_err(),
                "strict navigation action unexpectedly accepted {source}"
            );
        }
    }

    #[test]
    fn strict_recognition_and_page_dtos_reject_unknown_fields() {
        let recognition = r#"{
            "schema_version":"0.3",
            "targets":[{"type":"color","id":"target","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,0],"typo":true}]
        }"#;
        let pages = r#"{
            "schema_version":"0.3",
            "pages":[{"id":"game/home","required":["target"],"typo":true}]
        }"#;

        assert!(serde_json::from_str::<RawRecognitionDocument>(recognition).is_err());
        assert!(serde_json::from_str::<RawPageDocument>(pages).is_err());
    }

    fn admission_outcome(
        entries: &BTreeMap<String, Vec<u8>>,
        metadata: &PackageMetadata,
    ) -> String {
        match parse_package(entries, metadata) {
            Err(error) => format!("parse:{error}"),
            Ok(None) => "not_executable".to_string(),
            Ok(Some(parsed)) => match close_package(parsed, entries, &metadata.resource_root) {
                Err(error) => format!("close:{}", error.code()),
                Ok(closed) => match canonical_semantic_fingerprint(&closed) {
                    Ok(fingerprint) => format!("admitted:{fingerprint}"),
                    Err(error) => format!("fingerprint:{}", error.code()),
                },
            },
        }
    }

    fn next_fuzz_word(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn reverse_json_object_order(value: &mut Value) {
        match value {
            Value::Array(values) => {
                for value in values {
                    reverse_json_object_order(value);
                }
            }
            Value::Object(values) => {
                let mut entries = std::mem::take(values).into_iter().collect::<Vec<_>>();
                for (_, value) in &mut entries {
                    reverse_json_object_order(value);
                }
                entries.reverse();
                values.extend(entries);
            }
            _ => {}
        }
    }

    #[test]
    fn canonical_semantics_ignore_json_layout_and_equivalent_page_spelling() {
        let (mut entries, metadata) = parsed_package_fixture();
        mutate_json(&mut entries, "control.json", |control| {
            control["output"] = serde_json::json!({
                "z": {"second": 2, "first": 1},
                "a": [
                    {"right": true, "left": false}
                ]
            });
        });
        mutate_json(
            &mut entries,
            "resources/operations/task/task.json",
            |task| {
                task["provenance"] = serde_json::json!({
                    "version": "test",
                    "source": {"commit": "abc", "repository": "fixture"}
                });
            },
        );
        let baseline = admission_outcome(&entries, &metadata);

        let mut pretty = entries.clone();
        for bytes in pretty.values_mut() {
            let value: Value = serde_json::from_slice(bytes).expect("fixture JSON");
            *bytes = serde_json::to_vec_pretty(&value).expect("pretty fixture JSON");
        }
        assert_eq!(admission_outcome(&pretty, &metadata), baseline);

        let mut reordered = entries.clone();
        for bytes in reordered.values_mut() {
            let mut value: Value = serde_json::from_slice(bytes).expect("fixture JSON");
            reverse_json_object_order(&mut value);
            *bytes = serde_json::to_vec(&value).expect("reordered fixture JSON");
        }
        assert_eq!(admission_outcome(&reordered, &metadata), baseline);

        let mut unqualified_page_id = entries.clone();
        mutate_json(
            &mut unqualified_page_id,
            "resources/recognition/neutral.test.pages.json",
            |pages| pages["pages"][0]["id"] = Value::String("home".to_string()),
        );
        assert_eq!(admission_outcome(&unqualified_page_id, &metadata), baseline);

        let mut qualified = entries;
        mutate_json(
            &mut qualified,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["from"] = Value::String("neutral/home".to_string()),
        );
        assert_eq!(admission_outcome(&qualified, &metadata), baseline);
    }

    #[test]
    fn non_equivalent_canonical_operation_changes_the_semantic_fingerprint() {
        let (baseline, metadata) = parsed_package_fixture();
        let baseline = admission_outcome(&baseline, &metadata);
        let mut changed = parsed_package_fixture().0;
        mutate_json(
            &mut changed,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["destructive"] = Value::Bool(true),
        );
        let changed = admission_outcome(&changed, &metadata);

        assert!(baseline.starts_with("admitted:"), "{baseline}");
        assert!(changed.starts_with("admitted:"), "{changed}");
        assert_ne!(baseline, changed);
    }

    #[test]
    fn canonical_asset_bytes_change_the_semantic_fingerprint() {
        let (entries, metadata) = parsed_package_fixture();
        let mut closed = close_fixture(&entries, &metadata).expect("closed package");
        let asset = AssetKey::parse(
            "operations/task/assets/target.png".to_string(),
            "admission_asset_closure",
        )
        .expect("asset key");
        let baseline_bytes = Arc::<[u8]>::from([1_u8, 2, 3].as_slice());
        closed.assets.insert(
            asset.clone(),
            Sha256Hash::digest(&baseline_bytes).to_string(),
        );
        closed.asset_bytes.insert(asset.clone(), baseline_bytes);
        let baseline = canonical_semantic_fingerprint(&closed).expect("baseline fingerprint");

        let changed_bytes = Arc::from([1_u8, 2, 4].as_slice());
        closed.assets.insert(
            asset.clone(),
            Sha256Hash::digest(&changed_bytes).to_string(),
        );
        closed.asset_bytes.insert(asset, changed_bytes);
        let changed = canonical_semantic_fingerprint(&closed).expect("changed fingerprint");

        assert_ne!(baseline, changed);
    }

    #[test]
    fn retained_single_variable_mutation_corpus_has_explicit_oracles() {
        let (baseline_entries, metadata) = parsed_package_fixture();
        let baseline = admission_outcome(&baseline_entries, &metadata);
        assert!(baseline.starts_with("admitted:"), "{baseline}");

        let mut max_steps = baseline_entries.clone();
        mutate_json(&mut max_steps, "control.json", |control| {
            control["max_steps"] = Value::from(MAX_STEPS + 1);
        });
        assert_eq!(
            admission_outcome(&max_steps, &metadata),
            "close:admission_control_invalid"
        );

        let mut out_of_bounds = baseline_entries.clone();
        mutate_json(
            &mut out_of_bounds,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["click"]["x"] = Value::from(2),
        );
        assert_eq!(
            admission_outcome(&out_of_bounds, &metadata),
            "close:admission_input_bounds_invalid"
        );

        let mut missing_target = baseline_entries.clone();
        mutate_json(
            &mut missing_target,
            "resources/recognition/neutral.test.pages.json",
            |pages| {
                pages["pages"][0]["required"][0] = Value::String("page/missing".to_string());
            },
        );
        assert_eq!(
            admission_outcome(&missing_target, &metadata),
            "close:admission_missing_reference"
        );

        let mut unknown_safety_field = baseline_entries.clone();
        mutate_json(&mut unknown_safety_field, "control.json", |control| {
            control
                .as_object_mut()
                .expect("control object")
                .insert("unsafe_future_field".to_string(), Value::Bool(true));
        });
        assert!(
            admission_outcome(&unknown_safety_field, &metadata).starts_with("parse:"),
            "unknown safety field was not rejected at the wire boundary"
        );

        let mut typed_destructive = baseline_entries;
        mutate_json(
            &mut typed_destructive,
            "resources/operations/task/task.json",
            |task| task["operations"][0]["destructive"] = Value::Bool(true),
        );
        let typed_destructive = admission_outcome(&typed_destructive, &metadata);
        assert!(
            typed_destructive.starts_with("admitted:"),
            "{typed_destructive}"
        );
        assert_ne!(typed_destructive, baseline);
    }

    #[test]
    fn bounded_structured_mutation_is_deterministic_and_panic_free() {
        const CASES: usize = 256;
        let (baseline, metadata) = parsed_package_fixture();
        let mut state = 0x68_cafe_f00d_dead_u64;
        let started = std::time::Instant::now();

        for case in 0..CASES {
            let mut entries = baseline.clone();
            let word = next_fuzz_word(&mut state);
            match case % 8 {
                0 => mutate_json(&mut entries, "control.json", |control| {
                    control["max_steps"] = Value::from(word % 2_048);
                }),
                1 => mutate_json(
                    &mut entries,
                    "resources/operations/task/task.json",
                    |task| {
                        task["operations"][0]["click"]["x"] = Value::from((word as u32) as i32);
                    },
                ),
                2 => mutate_json(
                    &mut entries,
                    "resources/operations/task/task.json",
                    |task| {
                        task["operations"][0]["click"] = serde_json::json!({
                            "kind":"long_tap",
                            "x":1,
                            "y":0,
                            "duration_ms":word % 120_002
                        });
                    },
                ),
                3 => mutate_json(
                    &mut entries,
                    "resources/recognition/neutral.test.pack.json",
                    |pack| {
                        pack["targets"][0]["region"]["width"] = Value::from((word % 5) as i64 - 1);
                    },
                ),
                4 => mutate_json(
                    &mut entries,
                    "resources/recognition/neutral.test.pages.json",
                    |pages| {
                        pages["pages"][0]["required"][0] =
                            Value::String(format!("page/{:x}", word));
                    },
                ),
                5 => mutate_json(
                    &mut entries,
                    "resources/operations/task/task.json",
                    |task| {
                        task["operations"][0]["from"] = match word % 3 {
                            0 => Value::String("home".to_string()),
                            1 => Value::String("neutral/home".to_string()),
                            _ => Value::String("other/home".to_string()),
                        };
                    },
                ),
                6 => mutate_json(&mut entries, "control.json", |control| {
                    control
                        .as_object_mut()
                        .expect("control object")
                        .insert(format!("unknown_{word:x}"), Value::Bool(true));
                }),
                _ => mutate_json(
                    &mut entries,
                    "resources/navigation/neutral.test.navigation.json",
                    |navigation| {
                        navigation["control_points"] = serde_json::json!([{
                            "name":format!("point-{word:x}"),
                            "click":{"kind":"point","x":word as i64,"y":0}
                        }]);
                    },
                ),
            }

            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                admission_outcome(&entries, &metadata)
            }))
            .unwrap_or_else(|_| panic!("structured mutation case {case} panicked"));
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                admission_outcome(&entries, &metadata)
            }))
            .unwrap_or_else(|_| panic!("structured mutation replay case {case} panicked"));
            assert_eq!(first, second, "structured mutation case {case} drifted");
        }
        assert!(
            started.elapsed() <= std::time::Duration::from_secs(10),
            "bounded structured mutation exceeded its 10 second budget"
        );
    }
}
