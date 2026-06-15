// SPDX-License-Identifier: AGPL-3.0-only

//! Rust backup boundary for low-level device, capture, recognition, and input primitives.

use crate::types::*;

/// Execution-layer boundary mirrored from Go `PrimitiveLayer`.
pub trait PrimitiveLayer {
    fn connect_device(
        &mut self,
        ctx: &RuntimeContext,
        request: DeviceConnectRequest,
    ) -> ContractResult<DeviceSession>;

    fn start_app(
        &mut self,
        ctx: &RuntimeContext,
        request: AppRequest,
    ) -> ContractResult<ActionResult>;

    fn stop_app(
        &mut self,
        ctx: &RuntimeContext,
        request: AppRequest,
    ) -> ContractResult<ActionResult>;

    fn capture(
        &mut self,
        ctx: &RuntimeContext,
        request: CaptureRequest,
    ) -> ContractResult<CaptureRef>;

    fn match_templates(
        &mut self,
        ctx: &RuntimeContext,
        request: MatchRequest,
    ) -> ContractResult<MatchResult>;

    fn ocr(&mut self, ctx: &RuntimeContext, request: OcrRequest) -> ContractResult<OcrResult>;

    fn get_color(
        &mut self,
        ctx: &RuntimeContext,
        request: ColorRequest,
    ) -> ContractResult<ColorResult>;

    fn tap(&mut self, ctx: &RuntimeContext, request: TapRequest) -> ContractResult<ActionResult>;

    fn swipe(
        &mut self,
        ctx: &RuntimeContext,
        request: SwipeRequest,
    ) -> ContractResult<ActionResult>;

    fn wait_for(
        &mut self,
        ctx: &RuntimeContext,
        request: WaitForRequest,
    ) -> ContractResult<WaitForResult>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceConnectRequest {
    pub profile_id: ProfileId,
    pub device_id: String,
    pub backend: String,
    pub metadata: Metadata,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSession {
    pub id: String,
    pub device_id: String,
    pub backend: String,
    pub resolution: Resolution,
    pub connected_at: Timestamp,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppRequest {
    pub session_id: String,
    pub package: String,
    pub activity: Option<String>,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRequest {
    pub session_id: String,
    pub region: Option<Rect>,
    pub timeout_ms: DurationMillis,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRef {
    pub id: String,
    pub image_ref: String,
    pub image_hash: Option<String>,
    pub resolution: Resolution,
    pub captured_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchRequest {
    pub session_id: String,
    pub capture_id: Option<String>,
    pub templates: Vec<TemplateRef>,
    pub region: Option<Rect>,
    pub threshold: f64,
    pub max_results: Option<i32>,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateRef {
    pub id: String,
    pub path: String,
    pub hash: Option<String>,
    pub game: GameKey,
    pub server: ServerKey,
    pub locale: Option<String>,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    pub hits: Vec<MatchHit>,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchHit {
    pub template_id: String,
    pub score: f64,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrRequest {
    pub session_id: String,
    pub capture_id: Option<String>,
    pub region: Rect,
    pub languages: Vec<String>,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrResult {
    pub text: String,
    pub blocks: Vec<OcrBlock>,
    pub confidence: Option<f64>,
    pub observed_at: Timestamp,
    pub warnings: Vec<RuntimeError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrBlock {
    pub text: String,
    pub rect: Rect,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorRequest {
    pub session_id: String,
    pub capture_id: Option<String>,
    pub point: Point,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorResult {
    pub rgba: String,
    pub observed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TapRequest {
    pub session_id: String,
    pub point: Point,
    pub reason: String,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwipeRequest {
    pub session_id: String,
    pub from: Point,
    pub to: Point,
    pub duration_ms: DurationMillis,
    pub reason: String,
    pub timeout_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WaitForRequest {
    pub session_id: String,
    pub condition: String,
    pub timeout_ms: DurationMillis,
    pub poll_every_ms: DurationMillis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WaitForResult {
    pub satisfied: bool,
    pub observed_at: Timestamp,
    pub details: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionResult {
    pub ok: bool,
    pub observed_at: Timestamp,
    pub error: Option<RuntimeError>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
