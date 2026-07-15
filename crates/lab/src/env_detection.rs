// SPDX-License-Identifier: AGPL-3.0-only

use crate::{CaptureBackendFactory, Clock, ConfigSource, InputBackendFactory, Lab, LabPorts};
use actingcommand_contract::{EnvResolved, LabError, LabResult, NeedsDetection};
use actingcommand_device::{
    CaptureBackendConfig, Frame, InputBackend, PixelFormat, combine_operation_and_close,
};
use actingcommand_execution_kernel::{
    EnvCandidateMatcher, EnvDetectionCandidate, EnvDetectionCatalog, EnvDetectionKey,
    EnvDetectionStep, EnvDetectionStepPlan, EnvDetector, EnvironmentCandidateObservation,
    EnvironmentCatalogError, EnvironmentDecisionError, EnvironmentDetectionContext,
    EnvironmentDetectionEngine, EnvironmentDetectorState, EnvironmentKeyState,
    EnvironmentStateEngine, EnvironmentStateError, EnvironmentStateScope,
    canonical_environment_game, collect_environment_pointer_keys, default_environment_server,
    parse_environment_catalog_value,
};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{RecognitionEvaluator, load_pack_from_json_str};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

mod lock;
use lock::EnvResultLock;

const ENV_DETECTION_DIR: &str = "env-detection";
const ENV_DETECTION_CATALOG: &str = "detections.json";
const ENV_DETECTION_RESULT: &str = "result.json";
const ENV_DETECTION_SALT: &str = ".local_salt";
const ENV_INSTANCE_ID_PREFIX: &str = "envinst_";
const ENV_INSTANCE_ID_HASH_LEN: usize = 24;
static ENV_JSON_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

type EnvResult<T> = LabResult<T>;

pub use actingcommand_execution_kernel::{EnvDetectedValue, EnvDetectionResult};

#[cfg(test)]
use actingcommand_execution_kernel::{
    ENV_RESULT_SCHEMA_VERSION, EnvRect, validate_environment_value_safety,
};
#[cfg(test)]
use std::collections::BTreeMap;

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
        let detector = catalog
            .detector(&request.task)
            .map_err(environment_catalog_error)?;
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
            Some(&request.scope.instance),
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
        let detector = catalog
            .detector(&request.task)
            .map_err(environment_catalog_error)?;
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
        let detector = catalog
            .detector(&request.task)
            .map_err(environment_catalog_error)?;
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
    instance_alias: Option<&str>,
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
        capture_fresh_frame(lab, instance_alias, config, fresh_delay)?
    } else {
        let mut backend = lab
            .ports()
            .capture_factory()
            .open(crate::CaptureBackendRequest {
                instance_alias: instance_alias.map(str::to_string),
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
    instance_alias: Option<&str>,
    config: CaptureBackendConfig,
    delay: Duration,
) -> EnvResult<Frame> {
    let mut backend = lab
        .ports()
        .capture_factory()
        .open(crate::CaptureBackendRequest {
            instance_alias: instance_alias.map(str::to_string),
            config,
            observation: None,
        })?;
    let first = backend
        .capture()
        .map_err(|error| LabError::device(error.to_string()))?;
    lab.ports().clock().sleep(delay);
    let second = backend
        .capture()
        .map_err(|error| LabError::device(error.to_string()))?;
    if frame_digest(&first) != frame_digest(&second) {
        return Ok(second);
    }
    Err(LabError::device(
        "fresh capture required but the Runtime observation did not change",
    ))
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
    canonical_environment_game(value).map_err(environment_catalog_error)
}

fn default_server_for_game(game: &str) -> &'static str {
    default_environment_server(game)
}

trait EnvDetectionStepLabExt {
    fn to_plan(&self) -> EnvResult<EnvDetectionStepPlan>;
    fn to_direct_touch_command(&self) -> EnvResult<Option<crate::EnvTouchAction>> {
        Ok(match self.to_plan()? {
            EnvDetectionStepPlan::Tap { x, y } => Some(crate::EnvTouchAction::Tap { x, y }),
            EnvDetectionStepPlan::LongTap { x, y, duration_ms } => {
                Some(crate::EnvTouchAction::LongTap { x, y, duration_ms })
            }
            EnvDetectionStepPlan::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => Some(crate::EnvTouchAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            }),
            EnvDetectionStepPlan::Wait { .. } => None,
        })
    }
}

impl EnvDetectionStepLabExt for EnvDetectionStep {
    fn to_plan(&self) -> EnvResult<EnvDetectionStepPlan> {
        self.plan().map_err(environment_catalog_error)
    }
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
    catalog
        .select_detector_for_keys(requested_detector.as_deref(), keys)
        .map_err(environment_catalog_error)
}

fn parse_env_catalog_value(value: Value) -> Result<EnvDetectionCatalog, String> {
    parse_environment_catalog_value(value).map_err(|error| error.message().to_string())
}

fn validate_catalog(catalog: &EnvDetectionCatalog) -> EnvResult<()> {
    catalog.validate().map_err(environment_catalog_error)
}

#[cfg(test)]
fn validate_detection_step(
    detector: &EnvDetector,
    index: usize,
    step: &EnvDetectionStep,
) -> EnvResult<()> {
    detector
        .validate_step(index, step)
        .map_err(environment_catalog_error)
}

fn validate_detector_scope(detector: &EnvDetector, context: &EnvCommandContext) -> EnvResult<()> {
    detector
        .validate_scope(&context.game_id, &context.server_id)
        .map_err(environment_catalog_error)
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
                    instance_alias: Some(request.scope.instance.clone()),
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
            let duration_ms = step
                .required_duration()
                .map_err(environment_catalog_error)?;
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
    let mut observations = Vec::new();
    for key in &detector.keys {
        observations.extend(observe_detection_key(detector, key, context, scene)?);
    }
    EnvironmentDetectionEngine::decide(
        detector,
        &EnvironmentDetectionContext {
            instance_id: context.instance_id.clone(),
            game_id: context.game_id.clone(),
            server_id: context.server_id.clone(),
            resource_pack_hash: resource_hash.to_string(),
            generated_at_unix_ms: now_ms,
        },
        observations,
    )
    .map_err(environment_decision_error)
}

fn observe_detection_key(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
    context: &EnvCommandContext,
    scene: &Scene,
) -> EnvResult<Vec<EnvironmentCandidateObservation>> {
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
    key.candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            Ok(EnvironmentCandidateObservation {
                key: key.key.clone(),
                candidate_index: index,
                confidence: observe_candidate(key, candidate, index, scene, evaluator.as_ref())?,
            })
        })
        .collect()
}

fn observe_candidate(
    key: &EnvDetectionKey,
    candidate: &EnvDetectionCandidate,
    index: usize,
    scene: &Scene,
    evaluator: Option<&RecognitionEvaluator>,
) -> EnvResult<f32> {
    match candidate
        .matcher(&key.key)
        .map_err(environment_catalog_error)?
    {
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
            Ok(evaluation
                .template
                .as_ref()
                .map(|template| template.score)
                .unwrap_or(0.0))
        }
        EnvCandidateMatcher::SceneSize { width, height } => {
            Ok(if scene.width() == width && scene.height() == height {
                1.0
            } else {
                0.0
            })
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
        if candidate
            .matcher(&key.key)
            .map_err(environment_catalog_error)?
            != EnvCandidateMatcher::Template
        {
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
            resource_pack_id: detector.resource_pack_id(&context.game_id, &context.server_id),
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

#[cfg(test)]
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

#[cfg(test)]
fn validate_env_value_safety(value: &str, key: &str) -> EnvResult<()> {
    validate_environment_value_safety(value, key).map_err(environment_state_error)
}

fn environment_state_error(error: EnvironmentStateError) -> LabError {
    LabError::usage(error.message())
}

fn environment_catalog_error(error: EnvironmentCatalogError) -> LabError {
    LabError::usage(error.message())
}

fn environment_decision_error(error: EnvironmentDecisionError) -> LabError {
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
    let write = write_json_file_atomic(path, result);
    let release = lock.release();
    match (write, release) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(LabError::usage(format!(
            "{}; env detection lock release also failed: {}",
            error.message, release_error.message
        ))),
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
