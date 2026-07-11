// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CaptureBackendFactory, Clock, ConfigSource, InputBackendFactory, Lab, LabPorts};
use actingcommand_contract::{EnvResolved, LabError, LabResult, NeedsDetection};
use actingcommand_device::{
    CaptureBackendChoice, CaptureBackendConfig, Frame, InputBackend, PixelFormat,
    combine_operation_and_close,
};
use actingcommand_execution_kernel::{
    ENV_RESULT_SCHEMA_VERSION, EnvironmentDetectorState, EnvironmentKeyState,
    EnvironmentStateEngine, EnvironmentStateError, EnvironmentStateScope,
    collect_environment_pointer_keys, validate_environment_value_safety,
};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{RecognitionEvaluator, load_pack_from_json_str};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const ENV_DETECTION_DIR: &str = "env-detection";
const ENV_DETECTION_CATALOG: &str = "detections.json";
const ENV_DETECTION_RESULT: &str = "result.json";
const ENV_DETECTION_SALT: &str = ".local_salt";
const ENV_INSTANCE_ID_PREFIX: &str = "envinst_";
const ENV_INSTANCE_ID_HASH_LEN: usize = 24;
const ENV_DETECTION_MAX_STEP_DURATION_MS: u64 = 60_000;
static ENV_JSON_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

type EnvResult<T> = LabResult<T>;

pub use actingcommand_execution_kernel::{EnvDetectedValue, EnvDetectionResult};

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_json_file<T>(path: &Path) -> EnvResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .map_err(|error| LabError::usage(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| LabError::usage(format!("failed to parse {}: {error}", path.display())))
}

fn write_json_file_atomic<T: Serialize>(path: &Path, value: &T) -> EnvResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            LabError::usage(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| LabError::usage(format!("failed to serialize JSON: {error}")))?;
    cleanup_current_process_json_tmp_files(path)?;
    let sequence = ENV_JSON_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let mut file = File::create(&temporary).map_err(|error| {
        LabError::usage(format!("failed to create {}: {error}", temporary.display()))
    })?;
    file.write_all(text.as_bytes()).map_err(|error| {
        LabError::usage(format!("failed to write {}: {error}", temporary.display()))
    })?;
    file.sync_all().map_err(|error| {
        LabError::usage(format!("failed to sync {}: {error}", temporary.display()))
    })?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        LabError::usage(format!(
            "failed to publish {} from {}: {error}",
            path.display(),
            temporary.display()
        ))
    })
}

fn cleanup_current_process_json_tmp_files(path: &Path) -> EnvResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if !parent.exists() {
        return Ok(());
    }
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{stem}.tmp-{}-", std::process::id());
    for entry in fs::read_dir(parent)
        .map_err(|error| LabError::usage(format!("failed to read {}: {error}", parent.display())))?
    {
        let entry = entry.map_err(|error| {
            LabError::usage(format!("failed to inspect {}: {error}", parent.display()))
        })?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            fs::remove_file(entry.path()).map_err(|error| {
                LabError::usage(format!(
                    "failed to remove stale temp file {}: {error}",
                    entry.path().display()
                ))
            })?;
        }
    }
    Ok(())
}

impl<P: LabPorts> Lab<P> {
    pub fn detect_env(
        &mut self,
        request: crate::EnvDetectRequest,
    ) -> EnvResult<crate::EnvDetectResponse> {
        let context_now_ms = self.ports().clock().now_unix_ms()?;
        let context = EnvCommandContext::from_scope(&request.scope, context_now_ms)?;
        let catalog = load_env_catalog(&context.env_dir)?;
        let detector = catalog.detector(&request.task)?;
        validate_detector_scope(detector, &context)?;
        let step_run = run_detection_steps(self, &request, detector)?;
        if step_run.planned_only {
            return Ok(crate::EnvDetectResponse {
                schema_version: "env-detect-command.v1".to_string(),
                status: "planned".to_string(),
                dry_run: Some(true),
                task: detector.id.clone(),
                detector_id: detector.id.clone(),
                detector_version: detector.version().to_string(),
                instance_id: context.instance_id,
                game_id: context.game_id,
                server_id: context.server_id,
                resource_root: context.resource_root.display().to_string(),
                result_path: None,
                steps_executed: false,
                steps: step_run.steps,
                result: None,
            });
        }

        let scene = load_scene(
            self,
            request.scene_path.as_deref(),
            request.capture_config.as_ref(),
            request.require_fresh,
            request.fresh_delay,
            "command requires --scene <png> or --capture",
        )?;
        let now_ms = self.ports().clock().now_unix_ms()?;
        let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
        let result = evaluate_detector(detector, &context, &scene, &resource_hash, now_ms)?;
        let result_path = env_result_path(&context.env_dir, &context.instance_id);
        write_env_result(&result_path, &result)?;
        Ok(crate::EnvDetectResponse {
            schema_version: "env-detect-command.v1".to_string(),
            status: "detected".to_string(),
            dry_run: None,
            task: detector.id.clone(),
            detector_id: detector.id.clone(),
            detector_version: detector.version().to_string(),
            instance_id: context.instance_id,
            game_id: context.game_id,
            server_id: context.server_id,
            resource_root: context.resource_root.display().to_string(),
            result_path: Some(result_path.display().to_string()),
            steps_executed: step_run.executed,
            steps: step_run.steps,
            result: Some(result),
        })
    }

    pub fn resolve_env(
        &mut self,
        request: crate::EnvResolveRequest,
    ) -> EnvResult<crate::EnvResolveResponse> {
        let now_ms = self.ports().clock().now_unix_ms()?;
        let context = EnvCommandContext::from_scope(&request.scope, now_ms)?;
        let catalog = load_env_catalog(&context.env_dir)?;
        let detector = catalog.detector(&request.task)?;
        validate_detector_scope(detector, &context)?;
        let result_path = env_result_path(&context.env_dir, &context.instance_id);
        let Some(result) = load_optional_env_result(&result_path)? else {
            let details = needs_detection_details(
                detector,
                &context,
                &result_path,
                None,
                "missing_result",
                None,
            )?;
            return Err(LabError::usage(format!(
                "env detection result {} is missing; run detect first",
                result_path.display()
            ))
            .with_details(details));
        };
        let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
        if let Err(error) = ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms)
        {
            let reason = env_stale_reason(&error);
            let message = error.message.clone();
            let details = needs_detection_details(
                detector,
                &context,
                &result_path,
                Some(&result),
                reason,
                Some(&message),
            )?;
            return Err(error.with_details(details));
        }
        if request.input.is_none() && request.key.is_none() {
            return Err(LabError::usage(
                "env resolve requires --path <value-with-{env:key}> or --key <key>",
            ));
        }
        let (resolved, keys) = if let Some(input) = request.input {
            resolve_env_markers(&input, detector, &result, now_ms)?
        } else {
            let value = resolve_single_env_key(
                request.key.as_deref().unwrap_or_default(),
                detector,
                &result,
                now_ms,
            )?;
            (value.value.clone(), vec![value])
        };
        Ok(crate::EnvResolveResponse {
            schema_version: "env-resolve-command.v1".to_string(),
            status: "resolved".to_string(),
            task: detector.id.clone(),
            detector_id: detector.id.clone(),
            instance_id: context.instance_id,
            source_result: result_path.display().to_string(),
            resolved,
            keys,
        })
    }

    pub fn env_status(
        &mut self,
        request: crate::EnvStatusRequest,
    ) -> EnvResult<crate::EnvStatusResponse> {
        let now_ms = self.ports().clock().now_unix_ms()?;
        let context = EnvCommandContext::from_scope(&request.scope, now_ms)?;
        let catalog = load_env_catalog(&context.env_dir)?;
        let detector = catalog.detector(&request.task)?;
        validate_detector_scope(detector, &context)?;
        let result_path = env_result_path(&context.env_dir, &context.instance_id);
        let Some(result) = load_optional_env_result(&result_path)? else {
            return Ok(crate::EnvStatusResponse {
                schema_version: "env-status-command.v1".to_string(),
                status: "needs_detection".to_string(),
                reason: Some("missing_result".to_string()),
                task: detector.id.clone(),
                detector_id: Some(detector.id.clone()),
                detector_version: Some(detector.version().to_string()),
                instance_id: context.instance_id.clone(),
                result_path: result_path.display().to_string(),
                result: None,
                needs_detection: Some(needs_detection_payload(
                    detector,
                    &context,
                    &result_path,
                    None,
                    "missing_result",
                    None,
                )),
            });
        };
        let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
        let freshness = ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms);
        match freshness {
            Ok(()) => Ok(crate::EnvStatusResponse {
                schema_version: "env-status-command.v1".to_string(),
                status: "fresh".to_string(),
                reason: None,
                task: detector.id.clone(),
                detector_id: None,
                detector_version: None,
                instance_id: context.instance_id,
                result_path: result_path.display().to_string(),
                result: Some(result),
                needs_detection: None,
            }),
            Err(error) => {
                let reason = env_stale_reason(&error);
                Ok(crate::EnvStatusResponse {
                    schema_version: "env-status-command.v1".to_string(),
                    status: "stale".to_string(),
                    reason: Some(reason.to_string()),
                    task: detector.id.clone(),
                    detector_id: None,
                    detector_version: None,
                    instance_id: context.instance_id.clone(),
                    result_path: result_path.display().to_string(),
                    result: Some(result.clone()),
                    needs_detection: Some(needs_detection_payload(
                        detector,
                        &context,
                        &result_path,
                        Some(&result),
                        reason,
                        Some(&error.message),
                    )),
                })
            }
        }
    }
}

pub(crate) fn load_scene<P: LabPorts>(
    lab: &mut Lab<P>,
    scene_path: Option<&Path>,
    capture_config: Option<&CaptureBackendConfig>,
    require_fresh: bool,
    fresh_delay: Duration,
    missing_message: &str,
) -> EnvResult<Scene> {
    if let Some(path) = scene_path {
        let png = fs::read(path).map_err(|error| {
            LabError::device(format!("failed to read {}: {error}", path.display()))
        })?;
        return Scene::from_png(&png).map_err(|error| LabError::device(error.to_string()));
    }
    let config = capture_config
        .cloned()
        .ok_or_else(|| LabError::usage(missing_message))?;
    let frame = if require_fresh {
        capture_fresh_frame(lab, config, fresh_delay)?
    } else {
        let mut backend = lab
            .ports()
            .capture_factory()
            .open(crate::CaptureBackendRequest {
                config,
                observation: None,
            })?;
        backend
            .capture()
            .map_err(|error| LabError::device(error.to_string()))?
    };
    scene_from_frame(&frame)
}

fn capture_fresh_frame<P: LabPorts>(
    lab: &mut Lab<P>,
    config: CaptureBackendConfig,
    delay: Duration,
) -> EnvResult<Frame> {
    let choices = match config.requested {
        CaptureBackendChoice::Auto | CaptureBackendChoice::AutoFastest => vec![
            CaptureBackendChoice::NemuIpc,
            CaptureBackendChoice::DroidcastRaw,
            CaptureBackendChoice::Adb,
        ],
        other => vec![other],
    };
    let mut failures = Vec::new();
    for choice in choices {
        let mut choice_config = config.clone();
        choice_config.requested = choice;
        let mut backend = match lab
            .ports()
            .capture_factory()
            .open(crate::CaptureBackendRequest {
                config: choice_config,
                observation: None,
            }) {
            Ok(backend) => backend,
            Err(error) => {
                failures.push(format!("{} create: {}", choice.as_str(), error.message));
                continue;
            }
        };
        let first = match backend.capture() {
            Ok(frame) => frame,
            Err(error) => {
                failures.push(format!("{} first_capture: {error}", choice.as_str()));
                continue;
            }
        };
        lab.ports().clock().sleep(delay);
        let second = match backend.capture() {
            Ok(frame) => frame,
            Err(error) => {
                failures.push(format!("{} second_capture: {error}", choice.as_str()));
                continue;
            }
        };
        if frame_digest(&first) != frame_digest(&second) {
            return Ok(second);
        }
        failures.push(format!("{} expected_change_not_observed", choice.as_str()));
    }
    Err(LabError::device(format!(
        "fresh capture required but no backend produced a changing probe frame; attempts={}",
        serde_json::to_string(&failures).unwrap_or_else(|_| "[]".to_string())
    )))
}

fn scene_from_frame(frame: &Frame) -> EnvResult<Scene> {
    let pixel_format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, pixel_format)
        .map_err(|error| LabError::device(error.to_string()))
}

fn frame_digest(frame: &Frame) -> String {
    let mut hasher = Sha256::new();
    hasher.update(frame.width.to_le_bytes());
    hasher.update(frame.height.to_le_bytes());
    hasher.update(format!("{:?}", frame.pixel_format).as_bytes());
    hasher.update(&frame.pixels);
    format!("{:x}", hasher.finalize())
}

fn env_stale_reason(error: &LabError) -> &'static str {
    let message = error.message.as_str();
    if message.contains("schema") {
        "schema_mismatch"
    } else if message.contains("different instance_id") {
        "instance_mismatch"
    } else if message.contains("scope is stale") {
        "scope_mismatch"
    } else if message.contains("detector is stale") {
        "detector_mismatch"
    } else if message.contains("resource hash changed") {
        "resource_hash_changed"
    } else if message.contains("missing key") {
        "missing_key"
    } else if message.contains("confidence") && message.contains("below threshold") {
        "low_confidence"
    } else if message.contains("expired at") {
        "expired"
    } else if message.contains("unsafe value") {
        "unsafe_value"
    } else if message.contains("not in allowed_values") {
        "unallowed_value"
    } else {
        "stale_result"
    }
}

#[derive(Debug, Clone)]
struct EnvCommandContext {
    resource_root: PathBuf,
    env_dir: PathBuf,
    instance_id: String,
    game_id: String,
    server_id: String,
}

impl EnvCommandContext {
    fn from_scope(scope: &crate::EnvScopeRequest, now_ms: u64) -> EnvResult<Self> {
        if scope.instance.trim().is_empty() {
            return Err(LabError::usage("env detection requires --instance"));
        }
        let resource_root = resolve_resource_root(&scope.resource_root);
        let game_id = canonical_game(&scope.game)?;
        let server_id = scope
            .server
            .clone()
            .unwrap_or_else(|| default_server_for_game(&game_id).to_string());
        let env_dir = resource_root.join(ENV_DETECTION_DIR);
        let salt_dir = scope.state_root.join(ENV_DETECTION_DIR);
        let salt = read_or_create_local_salt(&salt_dir, now_ms)?;
        let instance_id = env_instance_id(&scope.instance, &salt)?;
        Ok(Self {
            resource_root,
            env_dir,
            instance_id,
            game_id,
            server_id,
        })
    }
}

fn needs_detection_details(
    detector: &EnvDetector,
    context: &EnvCommandContext,
    result_path: &Path,
    result: Option<&EnvDetectionResult>,
    reason: &'static str,
    error: Option<&str>,
) -> EnvResult<Value> {
    serde_json::to_value(needs_detection_payload(
        detector,
        context,
        result_path,
        result,
        reason,
        error,
    ))
    .map_err(|serialize_error| {
        LabError::device(format!(
            "failed to serialize needs-detection details: {serialize_error}"
        ))
    })
}

fn needs_detection_payload(
    detector: &EnvDetector,
    context: &EnvCommandContext,
    result_path: &Path,
    result: Option<&EnvDetectionResult>,
    reason: &'static str,
    error: Option<&str>,
) -> crate::EnvNeedsDetectionPayload {
    let detections = result
        .map(EnvDetectionResult::resolved_facts)
        .unwrap_or_default();
    let semantic = NeedsDetection {
        status: "needs_detection".to_string(),
        reason: reason.to_string(),
        command: Some("detect".to_string()),
        subject: Some(detector.id.clone()),
        detector_ids: vec![detector.id.clone()],
        keys: detections,
        recommended_action: "run_detect".to_string(),
    };
    crate::EnvNeedsDetectionPayload {
        status: semantic.status,
        reason: semantic.reason,
        task: detector.id.clone(),
        detector_id: detector.id.clone(),
        detector_version: detector.version().to_string(),
        instance_id: context.instance_id.clone(),
        game_id: context.game_id.clone(),
        server_id: context.server_id.clone(),
        result_path: result_path.display().to_string(),
        recommended_action: semantic.recommended_action,
        detections: semantic.keys,
        source_result: result
            .map(|result| format!("{}@{}", result.detector_id, result.generated_at_unix_ms)),
        result_generated_at_unix_ms: result.map(|result| result.generated_at_unix_ms),
        result_resource_pack_hash: result.map(|result| result.resource_pack_hash.clone()),
        error: error.map(str::to_string),
    }
}

fn resolve_resource_root(input: &Path) -> PathBuf {
    if looks_like_resource_root(input) {
        return input.to_path_buf();
    }
    let ours = input.join("ours");
    if looks_like_resource_root(&ours) {
        return ours;
    }
    input.to_path_buf()
}

fn looks_like_resource_root(path: &Path) -> bool {
    path.join("operations").is_dir()
        && (path.join("recognition").is_dir() || path.join("navigation").is_dir())
}

fn canonical_game(value: &str) -> EnvResult<String> {
    match value.to_ascii_lowercase().as_str() {
        "ak" | "ark" | "arknights" => Ok("arknights".to_string()),
        "azur" | "azurlane" | "azur_lane" | "al" => Ok("azurlane".to_string()),
        "ba" | "bluearchive" | "blue_archive" => Ok("bluearchive".to_string()),
        other => Err(LabError::usage(format!("unknown game selector: {other}"))),
    }
}

fn default_server_for_game(game: &str) -> &'static str {
    match game {
        "arknights" => "cn",
        "azurlane" | "bluearchive" => "jp",
        _ => "jp",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetectionCatalog {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default, alias = "detectors", alias = "tasks")]
    detections: Vec<EnvDetector>,
}

impl EnvDetectionCatalog {
    fn detector(&self, id: &str) -> EnvResult<&EnvDetector> {
        self.detections
            .iter()
            .find(|detector| detector.id == id)
            .ok_or_else(|| LabError::usage(format!("env detector '{id}' was not found")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetector {
    #[serde(alias = "task_id", alias = "detector_id")]
    id: String,
    #[serde(default, alias = "detector_version")]
    version: Option<String>,
    #[serde(default, alias = "game")]
    game_id: Option<String>,
    #[serde(default, alias = "server")]
    server_id: Option<String>,
    #[serde(default)]
    resource_pack_id: Option<String>,
    #[serde(default)]
    match_metric: Option<String>,
    #[serde(default, alias = "actions", alias = "pre_actions", alias = "pre_steps")]
    steps: Vec<EnvDetectionStep>,
    #[serde(alias = "outputs", alias = "items")]
    keys: Vec<EnvDetectionKey>,
}

impl EnvDetector {
    fn version(&self) -> &str {
        self.version.as_deref().unwrap_or("1")
    }

    fn resource_pack_id(&self, context: &EnvCommandContext) -> String {
        self.resource_pack_id
            .clone()
            .unwrap_or_else(|| format!("{}.{}", context.game_id, context.server_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EnvDetectionStep {
    #[serde(alias = "type", alias = "action")]
    kind: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    x1: Option<i32>,
    #[serde(default)]
    y1: Option<i32>,
    #[serde(default)]
    x2: Option<i32>,
    #[serde(default)]
    y2: Option<i32>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

impl EnvDetectionStep {
    fn canonical_kind(&self) -> EnvResult<&'static str> {
        match self.kind.trim() {
            "tap" => Ok("tap"),
            "long_tap" | "long-tap" | "longtap" => Ok("long_tap"),
            "swipe" => Ok("swipe"),
            "wait" | "sleep" => Ok("wait"),
            other => Err(LabError::usage(format!(
                "unsupported env detection step kind '{other}'"
            ))),
        }
    }

    fn to_direct_touch_command(&self) -> EnvResult<Option<crate::EnvTouchAction>> {
        match self.canonical_kind()? {
            "tap" => Ok(Some(crate::EnvTouchAction::Tap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
            })),
            "long_tap" => Ok(Some(crate::EnvTouchAction::LongTap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
                duration_ms: self.required_duration()?,
            })),
            "swipe" => Ok(Some(crate::EnvTouchAction::Swipe {
                x1: self.required_coord("x1")?,
                y1: self.required_coord("y1")?,
                x2: self.required_coord("x2")?,
                y2: self.required_coord("y2")?,
                duration_ms: self.required_duration()?,
            })),
            "wait" => Ok(None),
            _ => unreachable!(),
        }
    }

    fn required_coord(&self, name: &str) -> EnvResult<i32> {
        let value = match name {
            "x" => self.x,
            "y" => self.y,
            "x1" => self.x1,
            "y1" => self.y1,
            "x2" => self.x2,
            "y2" => self.y2,
            _ => None,
        }
        .ok_or_else(|| {
            LabError::usage(format!(
                "env detection step '{}' is missing coordinate {name}",
                self.kind
            ))
        })?;
        if value < 0 {
            return Err(LabError::usage(format!(
                "env detection step '{}' coordinate {name} must be non-negative",
                self.kind
            )));
        }
        Ok(value)
    }

    fn required_duration(&self) -> EnvResult<u64> {
        let duration_ms = self.duration_ms.ok_or_else(|| {
            LabError::usage(format!(
                "env detection step '{}' is missing duration_ms",
                self.kind
            ))
        })?;
        if duration_ms == 0 || duration_ms > ENV_DETECTION_MAX_STEP_DURATION_MS {
            return Err(LabError::usage(format!(
                "env detection step '{}' duration_ms must be in 1..={ENV_DETECTION_MAX_STEP_DURATION_MS}",
                self.kind
            )));
        }
        Ok(duration_ms)
    }

    fn to_plan(&self) -> EnvResult<crate::EnvDetectionStepPlan> {
        Ok(match self.canonical_kind()? {
            "tap" => crate::EnvDetectionStepPlan::Tap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
            },
            "long_tap" => crate::EnvDetectionStepPlan::LongTap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
                duration_ms: self.required_duration()?,
            },
            "swipe" => crate::EnvDetectionStepPlan::Swipe {
                x1: self.required_coord("x1")?,
                y1: self.required_coord("y1")?,
                x2: self.required_coord("x2")?,
                y2: self.required_coord("y2")?,
                duration_ms: self.required_duration()?,
            },
            "wait" => crate::EnvDetectionStepPlan::Wait {
                duration_ms: self.required_duration()?,
            },
            _ => unreachable!(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetectionKey {
    key: String,
    #[serde(alias = "threshold")]
    min_confidence: f32,
    #[serde(default, alias = "invalidate_below_confidence")]
    stale_below_confidence: Option<f32>,
    #[serde(default)]
    ttl_ms: Option<u64>,
    allowed_values: Vec<String>,
    candidates: Vec<EnvDetectionCandidate>,
}

impl EnvDetectionKey {
    fn stale_threshold(&self) -> f32 {
        self.stale_below_confidence.unwrap_or(self.min_confidence)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetectionCandidate {
    value: String,
    #[serde(default, alias = "template")]
    template_path: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(
        default,
        alias = "roi",
        deserialize_with = "deserialize_env_rect_option"
    )]
    region: Option<EnvRect>,
    #[serde(default)]
    threshold: Option<f32>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvCandidateMatcher {
    Template,
    SceneSize { width: u32, height: u32 },
}

impl EnvDetectionCandidate {
    fn matcher(&self, key: &str) -> EnvResult<EnvCandidateMatcher> {
        let template = self.template_path.as_deref().map(str::trim);
        let has_template = template.is_some_and(|value| !value.is_empty());
        let has_empty_template = template.is_some_and(str::is_empty);
        let has_width = self.width.is_some();
        let has_height = self.height.is_some();
        let has_scene_size = has_width || has_height;

        if has_empty_template {
            return Err(LabError::usage(format!(
                "env key '{}' candidate '{}' has empty template_path",
                key, self.value
            )));
        }
        if has_template && has_scene_size {
            return Err(LabError::usage(format!(
                "env key '{}' candidate '{}' must not mix template and scene size matchers",
                key, self.value
            )));
        }
        if has_template {
            return Ok(EnvCandidateMatcher::Template);
        }
        if has_width && has_height {
            let width = self.width.unwrap_or_default();
            let height = self.height.unwrap_or_default();
            if width == 0 || height == 0 {
                return Err(LabError::usage(format!(
                    "env key '{}' candidate '{}' scene size must be non-zero",
                    key, self.value
                )));
            }
            return Ok(EnvCandidateMatcher::SceneSize { width, height });
        }
        if has_scene_size {
            return Err(LabError::usage(format!(
                "env key '{}' candidate '{}' scene size matcher requires width and height",
                key, self.value
            )));
        }
        Err(LabError::usage(format!(
            "env key '{}' candidate '{}' must declare template_path or width/height",
            key, self.value
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct EnvRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

pub type ResolvedEnvValue = EnvResolved;

fn load_env_catalog(env_dir: &Path) -> EnvResult<EnvDetectionCatalog> {
    let path = env_dir.join(ENV_DETECTION_CATALOG);
    let text = fs::read_to_string(&path)
        .map_err(|err| LabError::usage(format!("failed to read {}: {err}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| LabError::usage(format!("failed to parse {}: {err}", path.display())))?;
    let catalog = parse_env_catalog_value(value).map_err(|err| {
        LabError::usage(format!(
            "failed to parse {} as env detection catalog: {err}",
            path.display()
        ))
    })?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

impl<P: LabPorts> Lab<P> {
    pub fn resolve_env_markers<T>(
        &mut self,
        request: crate::EnvMarkerResolutionRequest,
        value: &mut T,
    ) -> EnvResult<Vec<ResolvedEnvValue>>
    where
        T: Serialize + DeserializeOwned,
    {
        let mut json_value = serde_json::to_value(&*value).map_err(|error| {
            LabError::usage(format!("failed to serialize env marker input: {error}"))
        })?;
        let keys =
            collect_environment_pointer_keys(&json_value).map_err(environment_state_error)?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let now_ms = self.ports().clock().now_unix_ms()?;
        let state_root = self.ports().config().state_root()?;
        let resolved =
            resolve_env_markers_value(&request, &mut json_value, now_ms, state_root, keys)?;
        *value = serde_json::from_value(json_value).map_err(|error| {
            LabError::usage(format!("failed to deserialize env marker output: {error}"))
        })?;
        Ok(resolved)
    }
}

fn load_optional_env_result(path: &Path) -> EnvResult<Option<EnvDetectionResult>> {
    if !path.exists() {
        return Ok(None);
    }
    load_env_result(path).map(Some)
}

fn resolve_env_markers_value(
    request: &crate::EnvMarkerResolutionRequest,
    value: &mut Value,
    now_ms: u64,
    state_root: PathBuf,
    keys: BTreeSet<String>,
) -> EnvResult<Vec<ResolvedEnvValue>> {
    let scope = crate::EnvScopeRequest {
        resource_root: request.resource_root.clone(),
        state_root,
        instance: request
            .instance
            .clone()
            .ok_or_else(|| LabError::usage("env pointer resolution requires --instance"))?,
        game: request
            .game
            .clone()
            .ok_or_else(|| LabError::usage("env pointer resolution requires --game"))?,
        server: request.server.clone(),
    };
    let context = EnvCommandContext::from_scope(&scope, now_ms)?;
    let catalog = load_env_catalog(&context.env_dir)?;
    let detector = select_detector_for_env_keys(&catalog, request.env_task.clone(), &keys)?;
    validate_detector_scope(detector, &context)?;
    let result_path = env_result_path(&context.env_dir, &context.instance_id);
    let result = load_env_result(&result_path)?;
    let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
    ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms)?;
    environment_state_engine(detector, &context)
        .resolve_value(value, &result, now_ms)
        .map_err(environment_state_error)
}

fn select_detector_for_env_keys<'a>(
    catalog: &'a EnvDetectionCatalog,
    requested_detector: Option<String>,
    keys: &BTreeSet<String>,
) -> EnvResult<&'a EnvDetector> {
    if let Some(detector_id) = requested_detector {
        let detector = catalog.detector(&detector_id)?;
        ensure_detector_has_env_keys(detector, keys)?;
        return Ok(detector);
    }
    let matches = catalog
        .detections
        .iter()
        .filter(|detector| detector_declares_env_keys(detector, keys))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [detector] => Ok(detector),
        [] => Err(LabError::usage(format!(
            "no env detector declares all required keys: {}",
            keys.iter().cloned().collect::<Vec<_>>().join(", ")
        ))),
        _ => Err(LabError::usage(format!(
            "env keys are ambiguous across detectors; pass --env-task explicitly for keys: {}",
            keys.iter().cloned().collect::<Vec<_>>().join(", ")
        ))),
    }
}

fn ensure_detector_has_env_keys(detector: &EnvDetector, keys: &BTreeSet<String>) -> EnvResult<()> {
    if detector_declares_env_keys(detector, keys) {
        return Ok(());
    }
    Err(LabError::usage(format!(
        "env detector '{}' does not declare all required keys: {}",
        detector.id,
        keys.iter().cloned().collect::<Vec<_>>().join(", ")
    )))
}

fn detector_declares_env_keys(detector: &EnvDetector, keys: &BTreeSet<String>) -> bool {
    keys.iter()
        .all(|key| detector.keys.iter().any(|item| &item.key == key))
}

fn parse_env_catalog_value(value: Value) -> Result<EnvDetectionCatalog, String> {
    match serde_json::from_value::<EnvDetectionCatalog>(value.clone()) {
        Ok(catalog) => Ok(catalog),
        Err(structured_err) => normalize_flat_env_catalog(value).map_err(|flat_err| {
            format!("structured parse failed: {structured_err}; flat parse failed: {flat_err}")
        }),
    }
}

#[derive(Debug, Deserialize)]
struct FlatEnvDetectionCatalog {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default, alias = "game_id")]
    game: Option<String>,
    #[serde(default, alias = "server_id")]
    server: Option<String>,
    #[serde(default)]
    resource_pack_id: Option<String>,
    #[serde(default)]
    match_metric: Option<String>,
    #[serde(default)]
    detections: Vec<FlatEnvDetectionItem>,
}

#[derive(Debug, Deserialize)]
struct FlatEnvDetectionItem {
    #[serde(alias = "id", alias = "task_id")]
    detector_id: String,
    #[serde(default, alias = "version")]
    detector_version: Option<String>,
    #[serde(default, alias = "game", alias = "game_id")]
    game_id: Option<String>,
    #[serde(default, alias = "server", alias = "server_id")]
    server_id: Option<String>,
    #[serde(default)]
    resource_pack_id: Option<String>,
    #[serde(default)]
    match_metric: Option<String>,
    #[serde(default, alias = "actions", alias = "pre_actions", alias = "pre_steps")]
    steps: Vec<EnvDetectionStep>,
    #[serde(flatten)]
    key: EnvDetectionKey,
}

fn normalize_flat_env_catalog(value: Value) -> Result<EnvDetectionCatalog, String> {
    let flat: FlatEnvDetectionCatalog = serde_json::from_value(value)
        .map_err(|err| format!("invalid flat env detection catalog: {err}"))?;
    let mut detectors = BTreeMap::<String, EnvDetector>::new();
    for item in flat.detections {
        let detector_id = item.detector_id.trim().to_string();
        if detector_id.is_empty() {
            return Err("flat env detection item has an empty detector_id".to_string());
        }
        let candidate = EnvDetector {
            id: detector_id.clone(),
            version: item.detector_version,
            game_id: item.game_id.or_else(|| flat.game.clone()),
            server_id: item.server_id.or_else(|| flat.server.clone()),
            resource_pack_id: item
                .resource_pack_id
                .or_else(|| flat.resource_pack_id.clone()),
            match_metric: item.match_metric.or_else(|| flat.match_metric.clone()),
            steps: item.steps,
            keys: Vec::new(),
        };
        let detector = detectors
            .entry(detector_id.clone())
            .or_insert_with(|| candidate.clone());
        ensure_flat_detector_metadata_matches(detector, &candidate)?;
        detector.keys.push(item.key);
    }
    Ok(EnvDetectionCatalog {
        schema_version: flat.schema_version,
        detections: detectors.into_values().collect(),
    })
}

fn ensure_flat_detector_metadata_matches(
    current: &EnvDetector,
    candidate: &EnvDetector,
) -> Result<(), String> {
    let fields = [
        (
            "version",
            current.version.as_ref(),
            candidate.version.as_ref(),
        ),
        (
            "game_id",
            current.game_id.as_ref(),
            candidate.game_id.as_ref(),
        ),
        (
            "server_id",
            current.server_id.as_ref(),
            candidate.server_id.as_ref(),
        ),
        (
            "resource_pack_id",
            current.resource_pack_id.as_ref(),
            candidate.resource_pack_id.as_ref(),
        ),
        (
            "match_metric",
            current.match_metric.as_ref(),
            candidate.match_metric.as_ref(),
        ),
    ];
    for (field, left, right) in fields {
        if left != right {
            return Err(format!(
                "flat env detector '{}' has conflicting {field}",
                current.id
            ));
        }
    }
    if current.steps != candidate.steps {
        return Err(format!(
            "flat env detector '{}' has conflicting steps",
            current.id
        ));
    }
    Ok(())
}

fn validate_catalog(catalog: &EnvDetectionCatalog) -> EnvResult<()> {
    if let Some(schema_version) = &catalog.schema_version
        && schema_version != "env-detection.v1"
        && schema_version != "env-detections.v1"
    {
        return Err(LabError::usage(format!(
            "unsupported env detection schema_version '{schema_version}'"
        )));
    }
    let mut detector_ids = BTreeSet::new();
    for detector in &catalog.detections {
        if detector.id.trim().is_empty() {
            return Err(LabError::usage("env detector id must not be empty"));
        }
        if !detector_ids.insert(detector.id.clone()) {
            return Err(LabError::usage(format!(
                "env detector id '{}' is duplicated",
                detector.id
            )));
        }
        if detector.keys.is_empty() {
            return Err(LabError::usage(format!(
                "env detector '{}' must declare at least one key",
                detector.id
            )));
        }
        for (index, step) in detector.steps.iter().enumerate() {
            validate_detection_step(detector, index, step)?;
        }
        let mut key_ids = BTreeSet::new();
        for key in &detector.keys {
            validate_detection_key(detector, key)?;
            if !key_ids.insert(key.key.clone()) {
                return Err(LabError::usage(format!(
                    "env detector '{}' key '{}' is duplicated",
                    detector.id, key.key
                )));
            }
        }
    }
    Ok(())
}

fn validate_detection_step(
    detector: &EnvDetector,
    index: usize,
    step: &EnvDetectionStep,
) -> EnvResult<()> {
    step.to_plan().map_err(|err| {
        LabError::usage(format!(
            "env detector '{}' step {} is invalid: {}",
            detector.id,
            index + 1,
            err.message
        ))
    })?;
    Ok(())
}

fn deserialize_env_rect_option<'de, D>(deserializer: D) -> Result<Option<EnvRect>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    env_rect_from_value(&value).map_err(serde::de::Error::custom)
}

fn env_rect_from_value(value: &Value) -> Result<Option<EnvRect>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text == "full_frame" => Ok(None),
        Value::Object(_) => {
            let rect: EnvRect = serde_json::from_value(value.clone())
                .map_err(|err| format!("invalid env rect object: {err}"))?;
            Ok(Some(rect))
        }
        Value::Array(values) => {
            if values.len() != 4 {
                return Err("env roi array must contain exactly [x, y, width, height]".to_string());
            }
            let mut numbers = [0i32; 4];
            for (index, value) in values.iter().enumerate() {
                let number = value
                    .as_i64()
                    .ok_or_else(|| "env roi values must be integers".to_string())?;
                numbers[index] = i32::try_from(number)
                    .map_err(|_| "env roi value is outside i32 range".to_string())?;
            }
            Ok(Some(EnvRect {
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            }))
        }
        _ => Err("env region must be an object, null, full_frame, or roi array".to_string()),
    }
}

fn validate_detection_key(detector: &EnvDetector, key: &EnvDetectionKey) -> EnvResult<()> {
    if key.key.trim().is_empty() {
        return Err(LabError::usage(format!(
            "env detector '{}' has an empty key",
            detector.id
        )));
    }
    validate_confidence(key.min_confidence, &format!("{}.min_confidence", key.key))?;
    if let Some(threshold) = key.stale_below_confidence {
        validate_confidence(threshold, &format!("{}.stale_below_confidence", key.key))?;
    }
    if key.allowed_values.is_empty() {
        return Err(LabError::usage(format!(
            "env key '{}' must declare allowed_values",
            key.key
        )));
    }
    let allowed = key.allowed_values.iter().cloned().collect::<BTreeSet<_>>();
    if allowed.len() != key.allowed_values.len() {
        return Err(LabError::usage(format!(
            "env key '{}' allowed_values contains duplicate entries",
            key.key
        )));
    }
    for value in &key.allowed_values {
        validate_env_value_safety(value, &key.key)?;
    }
    if key.candidates.is_empty() {
        return Err(LabError::usage(format!(
            "env key '{}' must declare candidates",
            key.key
        )));
    }
    for candidate in &key.candidates {
        validate_env_value(candidate, key)?;
        candidate.matcher(&key.key)?;
        if let Some(threshold) = candidate.threshold {
            validate_confidence(threshold, &format!("{}.candidate.threshold", key.key))?;
        }
    }
    Ok(())
}

fn validate_confidence(value: f32, label: &str) -> EnvResult<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(LabError::usage(format!(
        "{label} must be finite and in 0.0..=1.0"
    )))
}

fn validate_detector_scope(detector: &EnvDetector, context: &EnvCommandContext) -> EnvResult<()> {
    if let Some(game) = &detector.game_id {
        let game = canonical_game(game)?;
        if game != context.game_id {
            return Err(LabError::usage(format!(
                "env detector '{}' is scoped to game '{}' but command game is '{}'",
                detector.id, game, context.game_id
            )));
        }
    }
    if let Some(server) = &detector.server_id
        && server != &context.server_id
    {
        return Err(LabError::usage(format!(
            "env detector '{}' is scoped to server '{}' but command server is '{}'",
            detector.id, server, context.server_id
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct EnvDetectionStepRun {
    planned_only: bool,
    executed: bool,
    steps: Vec<crate::EnvDetectionStepReport>,
}

fn run_detection_steps<P: LabPorts>(
    lab: &mut Lab<P>,
    request: &crate::EnvDetectRequest,
    detector: &EnvDetector,
) -> EnvResult<EnvDetectionStepRun> {
    let planned_steps = detector
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| -> EnvResult<_> {
            Ok(crate::EnvDetectionStepReport {
                index,
                status: None,
                step: step.to_plan()?,
                result: None,
            })
        })
        .collect::<EnvResult<Vec<_>>>()?;
    if detector.steps.is_empty() {
        return Ok(EnvDetectionStepRun {
            planned_only: false,
            executed: false,
            steps: planned_steps,
        });
    }
    if request.dry_run {
        return Ok(EnvDetectionStepRun {
            planned_only: true,
            executed: false,
            steps: planned_steps,
        });
    }
    if request.scene_path.is_some() || request.capture_config.is_none() {
        return Err(LabError::usage(format!(
            "env detector '{}' has interactive steps; execute it with --capture so recognition evaluates the post-step frame",
            detector.id
        )));
    }

    let mut steps = Vec::new();
    for (index, step) in detector.steps.iter().enumerate() {
        let planned = step.to_plan()?;
        let report = if let Some(action) = step.to_direct_touch_command()? {
            let config = request.touch_config.clone().ok_or_else(|| {
                LabError::device("env detection touch step requires device configuration")
            })?;
            let observation = crate::InputBackendObservation::default();
            let mut backend = lab
                .ports()
                .input_factory()
                .open(crate::InputBackendRequest {
                    config,
                    observation: Some(observation.clone()),
                })?;
            let operation = run_touch_action(&action, backend.as_mut());
            let close = backend.close();
            combine_operation_and_close(operation, close)
                .map_err(|error| LabError::device(error.to_string()))?;
            crate::EnvDetectionStepReport {
                index,
                status: Some("executed".to_string()),
                step: planned,
                result: Some(crate::EnvTouchResult {
                    status: "sent".to_string(),
                    backend: observation.snapshot()?,
                    control_mode: "env_detection_step".to_string(),
                    safety_gate: "declared_env_detection_step".to_string(),
                    action,
                }),
            }
        } else {
            let duration_ms = step.required_duration()?;
            lab.ports()
                .clock()
                .sleep(Duration::from_millis(duration_ms));
            crate::EnvDetectionStepReport {
                index,
                status: Some("executed".to_string()),
                step: planned,
                result: None,
            }
        };
        steps.push(report);
    }
    Ok(EnvDetectionStepRun {
        planned_only: false,
        executed: true,
        steps,
    })
}

fn run_touch_action(
    action: &crate::EnvTouchAction,
    backend: &mut dyn InputBackend,
) -> actingcommand_device::DeviceResult<()> {
    match action {
        crate::EnvTouchAction::Tap { x, y } => backend.tap(*x, *y),
        crate::EnvTouchAction::LongTap { x, y, duration_ms } => {
            backend.long_tap(*x, *y, *duration_ms)
        }
        crate::EnvTouchAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        } => backend.swipe(*x1, *y1, *x2, *y2, *duration_ms),
    }
}

fn evaluate_detector(
    detector: &EnvDetector,
    context: &EnvCommandContext,
    scene: &Scene,
    resource_hash: &str,
    now_ms: u64,
) -> EnvResult<EnvDetectionResult> {
    let mut detections = BTreeMap::new();
    for key in &detector.keys {
        let value = evaluate_detection_key(detector, key, context, scene, now_ms)?;
        detections.insert(key.key.clone(), value);
    }
    Ok(EnvDetectionResult {
        schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
        instance_id: context.instance_id.clone(),
        game_id: context.game_id.clone(),
        server_id: context.server_id.clone(),
        detector_id: detector.id.clone(),
        detector_version: detector.version().to_string(),
        resource_pack_id: detector.resource_pack_id(context),
        resource_pack_hash: resource_hash.to_string(),
        generated_at_unix_ms: now_ms,
        detections,
    })
}

fn evaluate_detection_key(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
    context: &EnvCommandContext,
    scene: &Scene,
    now_ms: u64,
) -> EnvResult<EnvDetectedValue> {
    let evaluator = if key.candidates.iter().any(|candidate| {
        matches!(
            candidate.matcher(&key.key),
            Ok(EnvCandidateMatcher::Template)
        )
    }) {
        Some(build_key_evaluator(detector, key, context, scene)?)
    } else {
        None
    };
    let mut best: Option<(&EnvDetectionCandidate, bool, f32)> = None;
    for (index, candidate) in key.candidates.iter().enumerate() {
        let (passed, score) = evaluate_candidate(key, candidate, index, scene, evaluator.as_ref())?;
        if best
            .as_ref()
            .is_none_or(|(_, _, best_score)| score > *best_score)
        {
            best = Some((candidate, passed, score));
        }
    }
    let Some((candidate, passed, confidence)) = best else {
        return Err(LabError::usage(format!(
            "env key '{}' has no evaluated candidates",
            key.key
        )));
    };
    if !passed || confidence < key.min_confidence {
        return Err(LabError::usage(format!(
            "env detector '{}' key '{}' needs detection: best candidate '{}' scored {:.6}, below threshold {:.6}",
            detector.id, key.key, candidate.value, confidence, key.min_confidence
        )));
    }
    validate_env_value(candidate, key)?;
    Ok(EnvDetectedValue {
        value: candidate.value.clone(),
        confidence,
        source: candidate
            .source
            .clone()
            .unwrap_or_else(|| format!("{}@{}", detector.id, candidate.value)),
        detected_at_unix_ms: now_ms,
        detector_id: detector.id.clone(),
        expires_at_unix_ms: key.ttl_ms.map(|ttl| now_ms.saturating_add(ttl)),
    })
}

fn evaluate_candidate(
    key: &EnvDetectionKey,
    candidate: &EnvDetectionCandidate,
    index: usize,
    scene: &Scene,
    evaluator: Option<&RecognitionEvaluator>,
) -> EnvResult<(bool, f32)> {
    match candidate.matcher(&key.key)? {
        EnvCandidateMatcher::Template => {
            let evaluator = evaluator.ok_or_else(|| {
                LabError::usage(format!(
                    "env key '{}' template candidate '{}' has no evaluator",
                    key.key, candidate.value
                ))
            })?;
            let target_id = env_target_id(&key.key, index);
            let evaluation = evaluator
                .evaluate_target(scene, &target_id)
                .map_err(|err| LabError::usage(err.to_string()))?;
            let score = evaluation
                .template
                .as_ref()
                .map(|template| template.score)
                .unwrap_or(0.0);
            Ok((evaluation.passed, score))
        }
        EnvCandidateMatcher::SceneSize { width, height } => {
            let confidence = if scene.width() == width && scene.height() == height {
                1.0
            } else {
                0.0
            };
            let threshold = candidate.threshold.unwrap_or(key.min_confidence);
            Ok((confidence >= threshold, confidence))
        }
    }
}

fn build_key_evaluator(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
    context: &EnvCommandContext,
    scene: &Scene,
) -> EnvResult<RecognitionEvaluator> {
    let mut targets = Vec::new();
    for (index, candidate) in key.candidates.iter().enumerate() {
        if candidate.matcher(&key.key)? != EnvCandidateMatcher::Template {
            continue;
        }
        let template_path = candidate.template_path.as_deref().ok_or_else(|| {
            LabError::usage(format!(
                "env key '{}' candidate '{}' has no template_path",
                key.key, candidate.value
            ))
        })?;
        let region = candidate.region.map_or_else(
            || json!("full_frame"),
            |rect| {
                json!({
                    "x": rect.x,
                    "y": rect.y,
                    "width": rect.width,
                    "height": rect.height
                })
            },
        );
        targets.push(json!({
                "type": "template",
                "id": env_target_id(&key.key, index),
            "template_path": template_path,
                "region": region,
                "threshold": candidate.threshold.unwrap_or(key.min_confidence),
                "mask": Value::Null,
                "rect_move": Value::Null,
                "color_check": Value::Null,
                "click": Value::Null
        }));
    }
    if targets.is_empty() {
        return Err(LabError::usage(format!(
            "env key '{}' has no template candidates",
            key.key
        )));
    }
    let match_metric = detector.match_metric.as_deref().unwrap_or("ccorr_normed");
    let pack_value = json!({
        "schema_version": "0.3",
        "game": context.game_id,
        "server": context.server_id,
        "coordinate_space": {
            "width": scene.width(),
            "height": scene.height()
        },
        "defaults": {
            "template_threshold": key.min_confidence,
            "match_metric": match_metric
        },
        "targets": targets
    });
    let pack_json = serde_json::to_string(&pack_value)
        .map_err(|err| LabError::usage(format!("failed to serialize env detection pack: {err}")))?;
    let pack =
        load_pack_from_json_str(&pack_json).map_err(|err| LabError::usage(err.to_string()))?;
    RecognitionEvaluator::new(context.resource_root.clone(), pack)
        .map_err(|err| LabError::usage(err.to_string()))
}

fn env_target_id(key: &str, index: usize) -> String {
    format!("env::{key}::{index}")
}

fn detector_resource_hash(detector: &EnvDetector, resource_root: &Path) -> EnvResult<String> {
    let mut hasher = Sha256::new();
    let detector_json = serde_json::to_vec(detector)
        .map_err(|err| LabError::usage(format!("failed to hash env detector: {err}")))?;
    hasher.update(&detector_json);
    let mut templates = detector
        .keys
        .iter()
        .flat_map(|key| {
            key.candidates
                .iter()
                .filter_map(|candidate| candidate.template_path.clone())
        })
        .collect::<Vec<_>>();
    templates.sort();
    templates.dedup();
    for template in templates {
        let path = resource_root.join(&template);
        let bytes = fs::read(&path).map_err(|err| {
            LabError::usage(format!(
                "failed to read env detector template {}: {err}",
                path.display()
            ))
        })?;
        hasher.update(template.as_bytes());
        hasher.update(hex_sha256(&bytes).as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn ensure_result_fresh(
    result: &EnvDetectionResult,
    detector: &EnvDetector,
    context: &EnvCommandContext,
    resource_hash: &str,
    now_ms: u64,
) -> EnvResult<()> {
    environment_state_engine(detector, context)
        .validate_result(result, resource_hash, now_ms)
        .map_err(environment_state_error)
}

fn resolve_env_markers(
    input: &str,
    detector: &EnvDetector,
    result: &EnvDetectionResult,
    now_ms: u64,
) -> EnvResult<(String, Vec<ResolvedEnvValue>)> {
    environment_state_engine_for_result(detector, result)
        .resolve_markers(input, result, now_ms)
        .map_err(environment_state_error)
}

fn resolve_single_env_key(
    key: &str,
    detector: &EnvDetector,
    result: &EnvDetectionResult,
    now_ms: u64,
) -> EnvResult<ResolvedEnvValue> {
    environment_state_engine_for_result(detector, result)
        .resolve_key(key, result, now_ms)
        .map_err(environment_state_error)
}

fn environment_state_engine(
    detector: &EnvDetector,
    context: &EnvCommandContext,
) -> EnvironmentStateEngine {
    EnvironmentStateEngine::new(
        EnvironmentStateScope {
            instance_id: context.instance_id.clone(),
            game_id: context.game_id.clone(),
            server_id: context.server_id.clone(),
            resource_pack_id: detector.resource_pack_id(context),
        },
        environment_detector_state(detector),
    )
}

fn environment_state_engine_for_result(
    detector: &EnvDetector,
    result: &EnvDetectionResult,
) -> EnvironmentStateEngine {
    EnvironmentStateEngine::new(
        EnvironmentStateScope {
            instance_id: result.instance_id.clone(),
            game_id: result.game_id.clone(),
            server_id: result.server_id.clone(),
            resource_pack_id: result.resource_pack_id.clone(),
        },
        environment_detector_state(detector),
    )
}

fn environment_detector_state(detector: &EnvDetector) -> EnvironmentDetectorState {
    EnvironmentDetectorState {
        id: detector.id.clone(),
        version: detector.version().to_string(),
        keys: detector
            .keys
            .iter()
            .map(|key| EnvironmentKeyState {
                key: key.key.clone(),
                stale_threshold: key.stale_threshold(),
                allowed_values: key.allowed_values.clone(),
            })
            .collect(),
    }
}

fn validate_env_value(candidate: &EnvDetectionCandidate, key: &EnvDetectionKey) -> EnvResult<()> {
    validate_env_value_safety(&candidate.value, &key.key)?;
    if key
        .allowed_values
        .iter()
        .any(|allowed| allowed == &candidate.value)
    {
        return Ok(());
    }
    Err(LabError::usage(format!(
        "env key '{}' candidate value '{}' is not in allowed_values",
        key.key, candidate.value
    )))
}

fn validate_env_value_safety(value: &str, key: &str) -> EnvResult<()> {
    validate_environment_value_safety(value, key).map_err(environment_state_error)
}

fn environment_state_error(error: EnvironmentStateError) -> LabError {
    LabError::usage(error.message())
}

fn env_result_path(env_dir: &Path, instance_id: &str) -> PathBuf {
    env_dir.join(instance_id).join(ENV_DETECTION_RESULT)
}

fn load_env_result(path: &Path) -> EnvResult<EnvDetectionResult> {
    read_json_file(path)?.ok_or_else(|| {
        LabError::usage(format!(
            "env detection result {} is missing; run detect first",
            path.display()
        ))
    })
}

fn write_env_result(path: &Path, result: &EnvDetectionResult) -> EnvResult<()> {
    let lock = EnvResultLock::acquire(path)?;
    write_json_file_atomic(path, result)?;
    lock.release()
}

#[derive(Debug)]
struct EnvResultLock {
    path: PathBuf,
    released: bool,
}

impl EnvResultLock {
    fn acquire(result_path: &Path) -> EnvResult<Self> {
        let lock_path = result_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                LabError::usage(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                file.write_all(format!("pid={}\n", std::process::id()).as_bytes())
                    .map_err(|err| {
                        LabError::usage(format!("failed to write {}: {err}", lock_path.display()))
                    })?;
                file.sync_all().map_err(|err| {
                    LabError::usage(format!("failed to sync {}: {err}", lock_path.display()))
                })?;
                Ok(Self {
                    path: lock_path,
                    released: false,
                })
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(LabError::usage(
                "env detection result lock conflict; another detection writer is active",
            )),
            Err(err) => Err(LabError::usage(format!(
                "failed to create env detection result lock {}: {err}",
                lock_path.display()
            ))),
        }
    }

    fn release(mut self) -> EnvResult<()> {
        fs::remove_file(&self.path).map_err(|err| {
            LabError::usage(format!(
                "failed to release env detection lock {}: {err}",
                self.path.display()
            ))
        })?;
        self.released = true;
        Ok(())
    }
}

impl Drop for EnvResultLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn read_or_create_local_salt(env_dir: &Path, now_ms: u64) -> EnvResult<String> {
    let path = env_dir.join(ENV_DETECTION_SALT);
    if path.exists() {
        return read_local_salt(&path);
    }
    fs::create_dir_all(env_dir)
        .map_err(|err| LabError::usage(format!("failed to create {}: {err}", env_dir.display())))?;
    let seed = format!("{}:{}:{}", now_ms, std::process::id(), env_dir.display());
    let salt = hex_sha256(seed.as_bytes());
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(salt.as_bytes()).map_err(|err| {
                LabError::usage(format!("failed to write {}: {err}", path.display()))
            })?;
            file.sync_all().map_err(|err| {
                LabError::usage(format!("failed to sync {}: {err}", path.display()))
            })?;
            Ok(salt)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => read_local_salt(&path),
        Err(err) => Err(LabError::usage(format!(
            "failed to create env detection salt {}: {err}",
            path.display()
        ))),
    }
}

fn read_local_salt(path: &Path) -> EnvResult<String> {
    let salt = fs::read_to_string(path)
        .map_err(|err| LabError::usage(format!("failed to read {}: {err}", path.display())))?;
    let salt = salt.trim().to_string();
    if salt.is_empty() {
        return Err(LabError::usage(format!(
            "env detection salt {} is empty",
            path.display()
        )));
    }
    Ok(salt)
}

fn env_instance_id(identity: &str, salt: &str) -> EnvResult<String> {
    let normalized = identity.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(LabError::usage("env detection instance identity is empty"));
    }
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hasher.update([0]);
    hasher.update(salt.as_bytes());
    let digest = hasher.finalize();
    Ok(format!(
        "{ENV_INSTANCE_ID_PREFIX}{}",
        &base32url_no_pad(&digest)[..ENV_INSTANCE_ID_HASH_LEN]
    ))
}

fn base32url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let mut buffer: u16 = 0;
    let mut bits: u8 = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((buffer >> bits) & 0x1f) as usize;
            out.push(ALPHABET[index] as char);
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[index] as char);
    }
    out
}

#[cfg(test)]
mod tests;
