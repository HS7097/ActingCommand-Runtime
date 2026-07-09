// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, DirectTouchCommand, FlagArgs, GlobalOptions, SemanticLedgerContext,
    app_state_root, canonical_game, current_unix_ms, default_server_for_game,
    effective_resource_root, finish_semantic_result_with_ledger, hex_sha256, load_scene_from_flags,
    read_json_file, read_user_config, resolve_resource_root, send_direct_touch_command,
    write_json_file_atomic,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    RecognitionEvaluator, TargetEvaluation, load_pack_from_json_str,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const ENV_DETECTION_DIR: &str = "env-detection";
const ENV_DETECTION_CATALOG: &str = "detections.json";
const ENV_DETECTION_RESULT: &str = "result.json";
const ENV_DETECTION_SALT: &str = ".local_salt";
const ENV_RESULT_SCHEMA_VERSION: &str = "env-detect-result.v1";
const ENV_INSTANCE_ID_PREFIX: &str = "envinst_";
const ENV_INSTANCE_ID_HASH_LEN: usize = 24;
const ENV_DETECTION_MAX_STEP_DURATION_MS: u64 = 60_000;

pub(super) fn run_detect(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let mut ledger = SemanticLedgerContext::new("detect", global, args);
    let result = (|| -> CliOutcome<Value> {
        let context = EnvCommandContext::from_flags(global, &flags)?;
        let detector_id = flags.required("--task")?;
        let catalog = load_env_catalog(&context.env_dir)?;
        let detector = catalog.detector(&detector_id)?;
        validate_detector_scope(detector, &context)?;
        let step_run = run_detection_steps(global, &flags, detector)?;
        if step_run.planned_only {
            ledger.record_drive(json!({
                "stage": "env_detection_steps_planned",
                "detector_id": detector.id,
                "detector_version": detector.version(),
                "instance_id": context.instance_id,
                "steps": step_run.steps
            }));
            return Ok(json!({
                "schema_version": "env-detect-command.v1",
                "status": "planned",
                "dry_run": true,
                "task": detector.id,
                "detector_id": detector.id,
                "detector_version": detector.version(),
                "instance_id": context.instance_id,
                "game_id": context.game_id,
                "server_id": context.server_id,
                "resource_root": context.resource_root.display().to_string(),
                "steps_executed": false,
                "steps": step_run.steps
            }));
        }
        let scene = load_scene_from_flags(global, &flags)?;
        let now_ms = current_unix_ms();
        let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
        let result = evaluate_detector(detector, &context, &scene, &resource_hash, now_ms)?;
        let result_path = env_result_path(&context.env_dir, &context.instance_id);
        write_env_result(&result_path, &result)?;
        ledger.record_drive(json!({
            "stage": "env_detected",
            "detector_id": detector.id,
            "detector_version": detector.version(),
            "instance_id": context.instance_id,
            "result_path": result_path.display().to_string(),
            "detections": result.detections.iter().map(|(key, value)| {
                json!({
                    "key": key,
                    "value": value.value,
                    "confidence": value.confidence,
                    "source": value.source
                })
            }).collect::<Vec<_>>()
        }));
        Ok(json!({
            "schema_version": "env-detect-command.v1",
            "status": "detected",
            "task": detector.id,
            "detector_id": detector.id,
            "detector_version": detector.version(),
            "instance_id": context.instance_id,
            "game_id": context.game_id,
            "server_id": context.server_id,
            "resource_root": context.resource_root.display().to_string(),
            "result_path": result_path.display().to_string(),
            "steps_executed": step_run.executed,
            "steps": step_run.steps,
            "result": result
        }))
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_env(
    subcommand: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    match subcommand {
        "resolve" => run_env_resolve(global, args),
        "status" => run_env_status(global, args),
        other => Err(CliError::usage(format!(
            "unknown env command: {other}; expected resolve or status"
        ))),
    }
}

fn run_env_resolve(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let mut ledger = SemanticLedgerContext::new("env-resolve", global, args);
    let result = (|| -> CliOutcome<Value> {
        let context = EnvCommandContext::from_flags(global, &flags)?;
        let detector_id = flags.required("--task")?;
        let catalog = load_env_catalog(&context.env_dir)?;
        let detector = catalog.detector(&detector_id)?;
        validate_detector_scope(detector, &context)?;
        let result_path = env_result_path(&context.env_dir, &context.instance_id);
        let result = load_env_result(&result_path)?;
        let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
        let now_ms = current_unix_ms();
        ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms)?;
        let input = flags
            .optional("--path")
            .or_else(|| flags.optional("--value"))
            .or_else(|| flags.positionals.first().cloned());
        let key = flags.optional("--key");
        if input.is_none() && key.is_none() {
            return Err(CliError::usage(
                "env resolve requires --path <value-with-{env:key}> or --key <key>",
            ));
        }
        let (resolved, resolved_keys) = if let Some(input) = input {
            resolve_env_markers(&input, detector, &result, now_ms)?
        } else {
            let key = key.unwrap_or_default();
            let value = resolve_single_env_key(&key, detector, &result, now_ms)?;
            (value.value.clone(), vec![value])
        };
        ledger.record_drive(json!({
            "stage": "env_resolved",
            "detector_id": detector.id,
            "instance_id": context.instance_id,
            "source_result": result_path.display().to_string(),
            "keys": resolved_keys.iter().map(|value| {
                json!({
                    "key": value.key,
                    "value": value.value,
                    "confidence": value.confidence,
                    "source": value.source
                })
            }).collect::<Vec<_>>()
        }));
        Ok(json!({
            "schema_version": "env-resolve-command.v1",
            "status": "resolved",
            "task": detector.id,
            "detector_id": detector.id,
            "instance_id": context.instance_id,
            "source_result": result_path.display().to_string(),
            "resolved": resolved,
            "keys": resolved_keys
        }))
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

fn run_env_status(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let context = EnvCommandContext::from_flags(global, &flags)?;
    let detector_id = flags.required("--task")?;
    let catalog = load_env_catalog(&context.env_dir)?;
    let detector = catalog.detector(&detector_id)?;
    validate_detector_scope(detector, &context)?;
    let result_path = env_result_path(&context.env_dir, &context.instance_id);
    let Some(result) = read_json_file::<EnvDetectionResult>(&result_path)? else {
        return Ok(json!({
            "schema_version": "env-status-command.v1",
            "status": "needs_detection",
            "reason": "missing_result",
            "task": detector.id,
            "instance_id": context.instance_id,
            "result_path": result_path.display().to_string()
        }));
    };
    let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
    let now_ms = current_unix_ms();
    let status = match ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms) {
        Ok(()) => "fresh",
        Err(_) => "stale",
    };
    Ok(json!({
        "schema_version": "env-status-command.v1",
        "status": status,
        "task": detector.id,
        "instance_id": context.instance_id,
        "result_path": result_path.display().to_string(),
        "result": result
    }))
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
    fn from_flags(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        let config = read_user_config()?;
        let resource_root = flags
            .optional_path("--resource-root")
            .map(|path| resolve_resource_root(&path).root)
            .or_else(|| effective_resource_root(global, &config))
            .ok_or_else(|| {
                CliError::usage("env detection requires --resource-root or config.resource_root")
            })?;
        let game_id = flags
            .optional("--game")
            .or_else(|| global.game.clone())
            .ok_or_else(|| CliError::usage("env detection requires --game"))?;
        let game_id = canonical_game(&game_id)?;
        let server_id = flags
            .optional("--server")
            .or_else(|| global.server.clone())
            .unwrap_or_else(|| default_server_for_game(&game_id).to_string());
        let instance_identity = flags
            .optional("--instance")
            .or_else(|| global.instance.clone())
            .ok_or_else(|| CliError::usage("env detection requires --instance"))?;
        let env_dir = resource_root.join(ENV_DETECTION_DIR);
        let salt_dir = app_state_root()?.join(ENV_DETECTION_DIR);
        let salt = read_or_create_local_salt(&salt_dir)?;
        let instance_id = env_instance_id(&instance_identity, &salt)?;
        Ok(Self {
            resource_root,
            env_dir,
            instance_id,
            game_id,
            server_id,
        })
    }

    fn from_resource_root(
        global: &GlobalOptions,
        flags: &FlagArgs,
        resource_root: &Path,
    ) -> CliOutcome<Self> {
        let game_id = flags
            .optional("--game")
            .or_else(|| global.game.clone())
            .ok_or_else(|| CliError::usage("env pointer resolution requires --game"))?;
        let game_id = canonical_game(&game_id)?;
        let server_id = flags
            .optional("--server")
            .or_else(|| global.server.clone())
            .unwrap_or_else(|| default_server_for_game(&game_id).to_string());
        let instance_identity = flags
            .optional("--instance")
            .or_else(|| global.instance.clone())
            .ok_or_else(|| CliError::usage("env pointer resolution requires --instance"))?;
        let env_dir = resource_root.join(ENV_DETECTION_DIR);
        let salt_dir = app_state_root()?.join(ENV_DETECTION_DIR);
        let salt = read_or_create_local_salt(&salt_dir)?;
        let instance_id = env_instance_id(&instance_identity, &salt)?;
        Ok(Self {
            resource_root: resource_root.to_path_buf(),
            env_dir,
            instance_id,
            game_id,
            server_id,
        })
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
    fn detector(&self, id: &str) -> CliOutcome<&EnvDetector> {
        self.detections
            .iter()
            .find(|detector| detector.id == id)
            .ok_or_else(|| CliError::usage(format!("env detector '{id}' was not found")))
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
    fn canonical_kind(&self) -> CliOutcome<&'static str> {
        match self.kind.trim() {
            "tap" => Ok("tap"),
            "long_tap" | "long-tap" | "longtap" => Ok("long_tap"),
            "swipe" => Ok("swipe"),
            "wait" | "sleep" => Ok("wait"),
            other => Err(CliError::usage(format!(
                "unsupported env detection step kind '{other}'"
            ))),
        }
    }

    fn requires_touch(&self) -> CliOutcome<bool> {
        Ok(match self.canonical_kind()? {
            "tap" | "long_tap" | "swipe" => true,
            "wait" => false,
            _ => unreachable!(),
        })
    }

    fn to_direct_touch_command(&self) -> CliOutcome<Option<DirectTouchCommand>> {
        match self.canonical_kind()? {
            "tap" => Ok(Some(DirectTouchCommand::Tap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
            })),
            "long_tap" => Ok(Some(DirectTouchCommand::LongTap {
                x: self.required_coord("x")?,
                y: self.required_coord("y")?,
                duration_ms: self.required_duration()?,
            })),
            "swipe" => Ok(Some(DirectTouchCommand::Swipe {
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

    fn required_coord(&self, name: &str) -> CliOutcome<i32> {
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
            CliError::usage(format!(
                "env detection step '{}' is missing coordinate {name}",
                self.kind
            ))
        })?;
        if value < 0 {
            return Err(CliError::usage(format!(
                "env detection step '{}' coordinate {name} must be non-negative",
                self.kind
            )));
        }
        Ok(value)
    }

    fn required_duration(&self) -> CliOutcome<u64> {
        let duration_ms = self.duration_ms.ok_or_else(|| {
            CliError::usage(format!(
                "env detection step '{}' is missing duration_ms",
                self.kind
            ))
        })?;
        if duration_ms == 0 || duration_ms > ENV_DETECTION_MAX_STEP_DURATION_MS {
            return Err(CliError::usage(format!(
                "env detection step '{}' duration_ms must be in 1..={ENV_DETECTION_MAX_STEP_DURATION_MS}",
                self.kind
            )));
        }
        Ok(duration_ms)
    }

    fn to_plan_json(&self) -> CliOutcome<Value> {
        Ok(match self.canonical_kind()? {
            "tap" => json!({
                "type": "tap",
                "x": self.required_coord("x")?,
                "y": self.required_coord("y")?
            }),
            "long_tap" => json!({
                "type": "long_tap",
                "x": self.required_coord("x")?,
                "y": self.required_coord("y")?,
                "duration_ms": self.required_duration()?
            }),
            "swipe" => json!({
                "type": "swipe",
                "x1": self.required_coord("x1")?,
                "y1": self.required_coord("y1")?,
                "x2": self.required_coord("x2")?,
                "y2": self.required_coord("y2")?,
                "duration_ms": self.required_duration()?
            }),
            "wait" => json!({
                "type": "wait",
                "duration_ms": self.required_duration()?
            }),
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
    #[serde(alias = "template")]
    template_path: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct EnvRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetectionResult {
    schema_version: String,
    instance_id: String,
    game_id: String,
    server_id: String,
    detector_id: String,
    detector_version: String,
    resource_pack_id: String,
    resource_pack_hash: String,
    generated_at_unix_ms: u64,
    detections: BTreeMap<String, EnvDetectedValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvDetectedValue {
    value: String,
    confidence: f32,
    source: String,
    detected_at_unix_ms: u64,
    detector_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResolvedEnvValue {
    pub(super) key: String,
    pub(super) value: String,
    pub(super) confidence: f32,
    pub(super) source: String,
    pub(super) source_result: String,
}

fn load_env_catalog(env_dir: &Path) -> CliOutcome<EnvDetectionCatalog> {
    let path = env_dir.join(ENV_DETECTION_CATALOG);
    let text = fs::read_to_string(&path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| CliError::usage(format!("failed to parse {}: {err}", path.display())))?;
    let catalog = parse_env_catalog_value(value).map_err(|err| {
        CliError::usage(format!(
            "failed to parse {} as env detection catalog: {err}",
            path.display()
        ))
    })?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub(super) fn resolve_env_markers_in_value(
    global: &GlobalOptions,
    flags: &FlagArgs,
    resource_root: &Path,
    value: &mut Value,
) -> CliOutcome<Vec<ResolvedEnvValue>> {
    let mut keys = BTreeSet::new();
    collect_env_pointer_keys(value, &mut keys)?;
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let context = EnvCommandContext::from_resource_root(global, flags, resource_root)?;
    let catalog = load_env_catalog(&context.env_dir)?;
    let detector = select_detector_for_env_keys(&catalog, flags.optional("--env-task"), &keys)?;
    validate_detector_scope(detector, &context)?;
    let result_path = env_result_path(&context.env_dir, &context.instance_id);
    let result = load_env_result(&result_path)?;
    let resource_hash = detector_resource_hash(detector, &context.resource_root)?;
    let now_ms = current_unix_ms();
    ensure_result_fresh(&result, detector, &context, &resource_hash, now_ms)?;
    let mut resolved = BTreeMap::<String, ResolvedEnvValue>::new();
    resolve_env_markers_in_value_inner(value, detector, &result, now_ms, &mut resolved)?;
    Ok(resolved.into_values().collect())
}

fn collect_env_pointer_keys(value: &Value, keys: &mut BTreeSet<String>) -> CliOutcome<()> {
    match value {
        Value::String(text) => {
            collect_env_pointer_keys_from_str(text, keys)?;
        }
        Value::Array(values) => {
            for value in values {
                collect_env_pointer_keys(value, keys)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_env_pointer_keys(value, keys)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn collect_env_pointer_keys_from_str(text: &str, keys: &mut BTreeSet<String>) -> CliOutcome<()> {
    let mut offset = 0usize;
    while let Some(start_rel) = text[offset..].find("{env:") {
        let key_start = offset + start_rel + "{env:".len();
        let end_rel = text[key_start..].find('}').ok_or_else(|| {
            CliError::usage(format!(
                "malformed env pointer in '{text}': missing closing '}}'"
            ))
        })?;
        let end = key_start + end_rel;
        let key = &text[key_start..end];
        if key.trim().is_empty() {
            return Err(CliError::usage("env pointer key must not be empty"));
        }
        keys.insert(key.to_string());
        offset = end + 1;
    }
    Ok(())
}

fn select_detector_for_env_keys<'a>(
    catalog: &'a EnvDetectionCatalog,
    requested_detector: Option<String>,
    keys: &BTreeSet<String>,
) -> CliOutcome<&'a EnvDetector> {
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
        [] => Err(CliError::usage(format!(
            "no env detector declares all required keys: {}",
            keys.iter().cloned().collect::<Vec<_>>().join(", ")
        ))),
        _ => Err(CliError::usage(format!(
            "env keys are ambiguous across detectors; pass --env-task explicitly for keys: {}",
            keys.iter().cloned().collect::<Vec<_>>().join(", ")
        ))),
    }
}

fn ensure_detector_has_env_keys(detector: &EnvDetector, keys: &BTreeSet<String>) -> CliOutcome<()> {
    if detector_declares_env_keys(detector, keys) {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "env detector '{}' does not declare all required keys: {}",
        detector.id,
        keys.iter().cloned().collect::<Vec<_>>().join(", ")
    )))
}

fn detector_declares_env_keys(detector: &EnvDetector, keys: &BTreeSet<String>) -> bool {
    keys.iter()
        .all(|key| detector.keys.iter().any(|item| &item.key == key))
}

fn resolve_env_markers_in_value_inner(
    value: &mut Value,
    detector: &EnvDetector,
    result: &EnvDetectionResult,
    now_ms: u64,
    resolved: &mut BTreeMap<String, ResolvedEnvValue>,
) -> CliOutcome<()> {
    match value {
        Value::String(text) => {
            let (replacement, keys) = resolve_env_markers(text, detector, result, now_ms)?;
            *text = replacement;
            for key in keys {
                resolved.entry(key.key.clone()).or_insert(key);
            }
        }
        Value::Array(values) => {
            for value in values {
                resolve_env_markers_in_value_inner(value, detector, result, now_ms, resolved)?;
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                resolve_env_markers_in_value_inner(value, detector, result, now_ms, resolved)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
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

fn validate_catalog(catalog: &EnvDetectionCatalog) -> CliOutcome<()> {
    if let Some(schema_version) = &catalog.schema_version
        && schema_version != "env-detection.v1"
        && schema_version != "env-detections.v1"
    {
        return Err(CliError::usage(format!(
            "unsupported env detection schema_version '{schema_version}'"
        )));
    }
    let mut detector_ids = BTreeSet::new();
    for detector in &catalog.detections {
        if detector.id.trim().is_empty() {
            return Err(CliError::usage("env detector id must not be empty"));
        }
        if !detector_ids.insert(detector.id.clone()) {
            return Err(CliError::usage(format!(
                "env detector id '{}' is duplicated",
                detector.id
            )));
        }
        if detector.keys.is_empty() {
            return Err(CliError::usage(format!(
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
                return Err(CliError::usage(format!(
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
) -> CliOutcome<()> {
    step.to_plan_json().map_err(|err| {
        CliError::usage(format!(
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

fn validate_detection_key(detector: &EnvDetector, key: &EnvDetectionKey) -> CliOutcome<()> {
    if key.key.trim().is_empty() {
        return Err(CliError::usage(format!(
            "env detector '{}' has an empty key",
            detector.id
        )));
    }
    validate_confidence(key.min_confidence, &format!("{}.min_confidence", key.key))?;
    if let Some(threshold) = key.stale_below_confidence {
        validate_confidence(threshold, &format!("{}.stale_below_confidence", key.key))?;
    }
    if key.allowed_values.is_empty() {
        return Err(CliError::usage(format!(
            "env key '{}' must declare allowed_values",
            key.key
        )));
    }
    let allowed = key.allowed_values.iter().cloned().collect::<BTreeSet<_>>();
    if allowed.len() != key.allowed_values.len() {
        return Err(CliError::usage(format!(
            "env key '{}' allowed_values contains duplicate entries",
            key.key
        )));
    }
    for value in &key.allowed_values {
        validate_env_value_safety(value, &key.key)?;
    }
    if key.candidates.is_empty() {
        return Err(CliError::usage(format!(
            "env key '{}' must declare candidates",
            key.key
        )));
    }
    for candidate in &key.candidates {
        validate_env_value(candidate, key)?;
        if candidate.template_path.trim().is_empty() {
            return Err(CliError::usage(format!(
                "env key '{}' candidate '{}' has empty template_path",
                key.key, candidate.value
            )));
        }
        if let Some(threshold) = candidate.threshold {
            validate_confidence(threshold, &format!("{}.candidate.threshold", key.key))?;
        }
    }
    Ok(())
}

fn validate_confidence(value: f32, label: &str) -> CliOutcome<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "{label} must be finite and in 0.0..=1.0"
    )))
}

fn validate_detector_scope(detector: &EnvDetector, context: &EnvCommandContext) -> CliOutcome<()> {
    if let Some(game) = &detector.game_id {
        let game = canonical_game(game)?;
        if game != context.game_id {
            return Err(CliError::usage(format!(
                "env detector '{}' is scoped to game '{}' but command game is '{}'",
                detector.id, game, context.game_id
            )));
        }
    }
    if let Some(server) = &detector.server_id
        && server != &context.server_id
    {
        return Err(CliError::usage(format!(
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
    steps: Vec<Value>,
}

fn run_detection_steps(
    global: &GlobalOptions,
    flags: &FlagArgs,
    detector: &EnvDetector,
) -> CliOutcome<EnvDetectionStepRun> {
    let planned_steps = detector
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            Ok(json!({
                "index": index,
                "step": step.to_plan_json()?
            }))
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    if detector.steps.is_empty() {
        return Ok(EnvDetectionStepRun {
            planned_only: false,
            executed: false,
            steps: planned_steps,
        });
    }
    if global.dry_run || flags.bool("--dry-run") {
        return Ok(EnvDetectionStepRun {
            planned_only: true,
            executed: false,
            steps: planned_steps,
        });
    }
    if flags.optional_path("--scene").is_some() || !flags.bool("--capture") {
        return Err(CliError::usage(format!(
            "env detector '{}' has interactive steps; execute it with --capture so recognition evaluates the post-step frame",
            detector.id
        )));
    }

    let needs_touch = detector
        .steps
        .iter()
        .map(EnvDetectionStep::requires_touch)
        .collect::<CliOutcome<Vec<_>>>()?
        .into_iter()
        .any(|value| value);
    let config = if needs_touch {
        Some(read_user_config()?)
    } else {
        None
    };
    let mut steps = Vec::new();
    for (index, step) in detector.steps.iter().enumerate() {
        let planned = step.to_plan_json()?;
        let status = if let Some(command) = step.to_direct_touch_command()? {
            let config = config.as_ref().ok_or_else(|| {
                CliError::device("env detection touch step requires device configuration")
            })?;
            let result = send_direct_touch_command(
                global,
                config,
                &command,
                "env_detection_step",
                "declared_env_detection_step",
            )?;
            json!({
                "index": index,
                "status": "executed",
                "step": planned,
                "result": result
            })
        } else {
            let duration_ms = step.required_duration()?;
            thread::sleep(Duration::from_millis(duration_ms));
            json!({
                "index": index,
                "status": "executed",
                "step": planned
            })
        };
        steps.push(status);
    }
    Ok(EnvDetectionStepRun {
        planned_only: false,
        executed: true,
        steps,
    })
}

fn evaluate_detector(
    detector: &EnvDetector,
    context: &EnvCommandContext,
    scene: &Scene,
    resource_hash: &str,
    now_ms: u64,
) -> CliOutcome<EnvDetectionResult> {
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
) -> CliOutcome<EnvDetectedValue> {
    let evaluator = build_key_evaluator(detector, key, context, scene)?;
    let mut best: Option<(&EnvDetectionCandidate, TargetEvaluation, f32)> = None;
    for (index, candidate) in key.candidates.iter().enumerate() {
        let target_id = env_target_id(&key.key, index);
        let evaluation = evaluator
            .evaluate_target(scene, &target_id)
            .map_err(|err| CliError::usage(err.to_string()))?;
        let score = evaluation
            .template
            .as_ref()
            .map(|template| template.score)
            .unwrap_or(0.0);
        if best
            .as_ref()
            .is_none_or(|(_, _, best_score)| score > *best_score)
        {
            best = Some((candidate, evaluation, score));
        }
    }
    let Some((candidate, evaluation, confidence)) = best else {
        return Err(CliError::usage(format!(
            "env key '{}' has no evaluated candidates",
            key.key
        )));
    };
    if !evaluation.passed || confidence < key.min_confidence {
        return Err(CliError::usage(format!(
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

fn build_key_evaluator(
    detector: &EnvDetector,
    key: &EnvDetectionKey,
    context: &EnvCommandContext,
    scene: &Scene,
) -> CliOutcome<RecognitionEvaluator> {
    let targets = key
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
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
            json!({
                "type": "template",
                "id": env_target_id(&key.key, index),
                "template_path": candidate.template_path,
                "region": region,
                "threshold": candidate.threshold.unwrap_or(key.min_confidence),
                "mask": Value::Null,
                "rect_move": Value::Null,
                "color_check": Value::Null,
                "click": Value::Null
            })
        })
        .collect::<Vec<_>>();
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
        .map_err(|err| CliError::usage(format!("failed to serialize env detection pack: {err}")))?;
    let pack =
        load_pack_from_json_str(&pack_json).map_err(|err| CliError::usage(err.to_string()))?;
    RecognitionEvaluator::new(context.resource_root.clone(), pack)
        .map_err(|err| CliError::usage(err.to_string()))
}

fn env_target_id(key: &str, index: usize) -> String {
    format!("env::{key}::{index}")
}

fn detector_resource_hash(detector: &EnvDetector, resource_root: &Path) -> CliOutcome<String> {
    let mut hasher = Sha256::new();
    let detector_json = serde_json::to_vec(detector)
        .map_err(|err| CliError::usage(format!("failed to hash env detector: {err}")))?;
    hasher.update(&detector_json);
    let mut templates = detector
        .keys
        .iter()
        .flat_map(|key| {
            key.candidates
                .iter()
                .map(|candidate| candidate.template_path.clone())
        })
        .collect::<Vec<_>>();
    templates.sort();
    templates.dedup();
    for template in templates {
        let path = resource_root.join(&template);
        let bytes = fs::read(&path).map_err(|err| {
            CliError::usage(format!(
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
) -> CliOutcome<()> {
    if result.schema_version != ENV_RESULT_SCHEMA_VERSION {
        return Err(CliError::usage(format!(
            "env detection result schema '{}' is stale; expected '{}'",
            result.schema_version, ENV_RESULT_SCHEMA_VERSION
        )));
    }
    if result.instance_id != context.instance_id {
        return Err(CliError::usage(
            "env detection result belongs to a different instance_id",
        ));
    }
    if result.game_id != context.game_id || result.server_id != context.server_id {
        return Err(CliError::usage(format!(
            "env detection result scope is stale: result {}.{} command {}.{}",
            result.game_id, result.server_id, context.game_id, context.server_id
        )));
    }
    if result.detector_id != detector.id || result.detector_version != detector.version() {
        return Err(CliError::usage(format!(
            "env detection result detector is stale: result {}@{} command {}@{}",
            result.detector_id,
            result.detector_version,
            detector.id,
            detector.version()
        )));
    }
    if result.resource_pack_id != detector.resource_pack_id(context)
        || result.resource_pack_hash != resource_hash
    {
        return Err(CliError::usage(
            "env detection result is stale because detector resource hash changed",
        ));
    }
    for key in &detector.keys {
        let value = result.detections.get(&key.key).ok_or_else(|| {
            CliError::usage(format!(
                "env detection result is missing key '{}'; run detect first",
                key.key
            ))
        })?;
        validate_resolved_value(&key.key, value, key, now_ms)?;
    }
    Ok(())
}

fn resolve_env_markers(
    input: &str,
    detector: &EnvDetector,
    result: &EnvDetectionResult,
    now_ms: u64,
) -> CliOutcome<(String, Vec<ResolvedEnvValue>)> {
    let mut resolved = String::new();
    let mut keys = Vec::new();
    let mut offset = 0usize;
    while let Some(start_rel) = input[offset..].find("{env:") {
        let start = offset + start_rel;
        resolved.push_str(&input[offset..start]);
        let key_start = start + "{env:".len();
        let end_rel = input[key_start..].find('}').ok_or_else(|| {
            CliError::usage(format!(
                "malformed env pointer in '{input}': missing closing '}}'"
            ))
        })?;
        let end = key_start + end_rel;
        let key = &input[key_start..end];
        let value = resolve_single_env_key(key, detector, result, now_ms)?;
        resolved.push_str(&value.value);
        keys.push(value);
        offset = end + 1;
    }
    resolved.push_str(&input[offset..]);
    Ok((resolved, keys))
}

fn resolve_single_env_key(
    key: &str,
    detector: &EnvDetector,
    result: &EnvDetectionResult,
    now_ms: u64,
) -> CliOutcome<ResolvedEnvValue> {
    let key_config = detector
        .keys
        .iter()
        .find(|item| item.key == key)
        .ok_or_else(|| {
            CliError::usage(format!(
                "env key '{key}' is not declared by detector '{}'",
                detector.id
            ))
        })?;
    let value = result.detections.get(key).ok_or_else(|| {
        CliError::usage(format!(
            "env detection result is missing key '{key}'; run detect first"
        ))
    })?;
    validate_resolved_value(key, value, key_config, now_ms)?;
    Ok(ResolvedEnvValue {
        key: key.to_string(),
        value: value.value.clone(),
        confidence: value.confidence,
        source: value.source.clone(),
        source_result: format!("{}@{}", result.detector_id, result.generated_at_unix_ms),
    })
}

fn validate_resolved_value(
    key: &str,
    value: &EnvDetectedValue,
    key_config: &EnvDetectionKey,
    now_ms: u64,
) -> CliOutcome<()> {
    validate_env_value_safety(&value.value, key)?;
    if !key_config
        .allowed_values
        .iter()
        .any(|allowed| allowed == &value.value)
    {
        return Err(CliError::usage(format!(
            "env key '{key}' value '{}' is not in allowed_values",
            value.value
        )));
    }
    if value.confidence < key_config.stale_threshold() {
        return Err(CliError::usage(format!(
            "env key '{key}' is stale: confidence {:.6} below threshold {:.6}",
            value.confidence,
            key_config.stale_threshold()
        )));
    }
    if let Some(expires_at) = value.expires_at_unix_ms
        && now_ms > expires_at
    {
        return Err(CliError::usage(format!(
            "env key '{key}' expired at {expires_at}; run detect first"
        )));
    }
    Ok(())
}

fn validate_env_value(candidate: &EnvDetectionCandidate, key: &EnvDetectionKey) -> CliOutcome<()> {
    validate_env_value_safety(&candidate.value, &key.key)?;
    if key
        .allowed_values
        .iter()
        .any(|allowed| allowed == &candidate.value)
    {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "env key '{}' candidate value '{}' is not in allowed_values",
        key.key, candidate.value
    )))
}

fn validate_env_value_safety(value: &str, key: &str) -> CliOutcome<()> {
    if value.is_empty()
        || value == "."
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains("..")
        || Path::new(value).is_absolute()
    {
        return Err(CliError::usage(format!(
            "env key '{key}' has unsafe value '{value}'"
        )));
    }
    Ok(())
}

fn env_result_path(env_dir: &Path, instance_id: &str) -> PathBuf {
    env_dir.join(instance_id).join(ENV_DETECTION_RESULT)
}

fn load_env_result(path: &Path) -> CliOutcome<EnvDetectionResult> {
    read_json_file(path)?.ok_or_else(|| {
        CliError::usage(format!(
            "env detection result {} is missing; run detect first",
            path.display()
        ))
    })
}

fn write_env_result(path: &Path, result: &EnvDetectionResult) -> CliOutcome<()> {
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
    fn acquire(result_path: &Path) -> CliOutcome<Self> {
        let lock_path = result_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                CliError::usage(format!("failed to create {}: {err}", parent.display()))
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
                        CliError::usage(format!("failed to write {}: {err}", lock_path.display()))
                    })?;
                file.sync_all().map_err(|err| {
                    CliError::usage(format!("failed to sync {}: {err}", lock_path.display()))
                })?;
                Ok(Self {
                    path: lock_path,
                    released: false,
                })
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(CliError::usage(
                "env detection result lock conflict; another detection writer is active",
            )),
            Err(err) => Err(CliError::usage(format!(
                "failed to create env detection result lock {}: {err}",
                lock_path.display()
            ))),
        }
    }

    fn release(mut self) -> CliOutcome<()> {
        fs::remove_file(&self.path).map_err(|err| {
            CliError::usage(format!(
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

fn read_or_create_local_salt(env_dir: &Path) -> CliOutcome<String> {
    let path = env_dir.join(ENV_DETECTION_SALT);
    if path.exists() {
        return read_local_salt(&path);
    }
    fs::create_dir_all(env_dir)
        .map_err(|err| CliError::usage(format!("failed to create {}: {err}", env_dir.display())))?;
    let seed = format!(
        "{}:{}:{}",
        current_unix_ms(),
        std::process::id(),
        env_dir.display()
    );
    let salt = hex_sha256(seed.as_bytes());
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(salt.as_bytes()).map_err(|err| {
                CliError::usage(format!("failed to write {}: {err}", path.display()))
            })?;
            file.sync_all().map_err(|err| {
                CliError::usage(format!("failed to sync {}: {err}", path.display()))
            })?;
            Ok(salt)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => read_local_salt(&path),
        Err(err) => Err(CliError::usage(format!(
            "failed to create env detection salt {}: {err}",
            path.display()
        ))),
    }
}

fn read_local_salt(path: &Path) -> CliOutcome<String> {
    let salt = fs::read_to_string(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    let salt = salt.trim().to_string();
    if salt.is_empty() {
        return Err(CliError::usage(format!(
            "env detection salt {} is empty",
            path.display()
        )));
    }
    Ok(salt)
}

fn env_instance_id(identity: &str, salt: &str) -> CliOutcome<String> {
    let normalized = identity.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(CliError::usage("env detection instance identity is empty"));
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
mod tests {
    use super::*;
    use actingcommand_recognition::ScenePixelFormat;
    use tempfile::TempDir;

    #[test]
    fn instance_id_is_stable_safe_and_desensitized() {
        let first = env_instance_id("127.0.0.1:16416", "salt").unwrap();
        let second = env_instance_id(" 127.0.0.1:16416 ", "salt").unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(ENV_INSTANCE_ID_PREFIX));
        assert!(!first.contains(':'));
        assert!(!first.contains('/'));
        assert!(!first.contains('\\'));
        assert!(!first.contains("127.0.0.1"));
    }

    #[test]
    fn unsafe_or_unlisted_values_are_rejected() {
        let key = EnvDetectionKey {
            key: "ui_theme".to_string(),
            min_confidence: 0.7,
            stale_below_confidence: None,
            ttl_ms: None,
            allowed_values: vec!["Default".to_string()],
            candidates: Vec::new(),
        };
        for value in ["", "../bad", "bad/name", "bad\\name", "C:bad", "bad..name"] {
            let candidate = candidate(value);
            assert!(validate_env_value(&candidate, &key).is_err());
        }
        let other_candidate = candidate("Other");
        assert!(validate_env_value(&other_candidate, &key).is_err());
        let default_candidate = candidate("Default");
        assert!(validate_env_value(&default_candidate, &key).is_ok());
    }

    #[test]
    fn result_path_uses_instance_id_not_raw_endpoint() {
        let path = env_result_path(Path::new("ours/env-detection"), "envinst_abc");
        let text = path.display().to_string();
        assert!(text.contains("envinst_abc"));
        assert!(!text.contains("127.0.0.1"));
        assert_eq!(
            path.file_name().and_then(|value| value.to_str()),
            Some("result.json")
        );
    }

    #[test]
    fn flat_resource_authored_catalog_is_normalized() {
        let temp = TempDir::new().unwrap();
        let env_dir = temp.path().join(ENV_DETECTION_DIR);
        fs::create_dir_all(&env_dir).unwrap();
        fs::write(
            env_dir.join(ENV_DETECTION_CATALOG),
            r#"{
                "schema_version": "env-detections.v1",
                "game": "arknights",
                "detections": [{
                    "detector_id": "detect_ui_theme",
                    "detector_version": "1",
                    "key": "ui_theme",
                    "method": "any_of",
                    "threshold": 0.7,
                    "invalidate_below_confidence": 0.6,
                    "ttl_ms": null,
                    "allowed_values": ["Default"],
                    "candidates": [{
                        "value": "Default",
                        "template": "hometheme/Default/Terminal.png",
                        "roi": [844, 58, 268, 272]
                    }]
                }]
            }"#,
        )
        .unwrap();

        let catalog = load_env_catalog(&env_dir).unwrap();
        let detector = catalog.detector("detect_ui_theme").unwrap();
        assert_eq!(detector.game_id.as_deref(), Some("arknights"));
        assert_eq!(detector.version(), "1");
        assert_eq!(detector.keys.len(), 1);
        let key = &detector.keys[0];
        assert_eq!(key.key, "ui_theme");
        assert_eq!(key.min_confidence, 0.7);
        assert_eq!(key.stale_below_confidence, Some(0.6));
        let candidate = &key.candidates[0];
        assert_eq!(candidate.template_path, "hometheme/Default/Terminal.png");
        assert_eq!(
            candidate.region,
            Some(EnvRect {
                x: 844,
                y: 58,
                width: 268,
                height: 272
            })
        );
    }

    #[test]
    fn interactive_steps_are_data_defined_and_validated() {
        let mut detector = detector();
        detector.steps = vec![
            EnvDetectionStep {
                kind: "tap".to_string(),
                x: Some(100),
                y: Some(200),
                x1: None,
                y1: None,
                x2: None,
                y2: None,
                duration_ms: None,
            },
            EnvDetectionStep {
                kind: "long-tap".to_string(),
                x: Some(110),
                y: Some(210),
                x1: None,
                y1: None,
                x2: None,
                y2: None,
                duration_ms: Some(500),
            },
            EnvDetectionStep {
                kind: "swipe".to_string(),
                x: None,
                y: None,
                x1: Some(10),
                y1: Some(20),
                x2: Some(30),
                y2: Some(40),
                duration_ms: Some(300),
            },
            EnvDetectionStep {
                kind: "wait".to_string(),
                x: None,
                y: None,
                x1: None,
                y1: None,
                x2: None,
                y2: None,
                duration_ms: Some(1),
            },
        ];
        for (index, step) in detector.steps.iter().enumerate() {
            validate_detection_step(&detector, index, step).unwrap();
        }
        assert_eq!(
            detector.steps[1].to_plan_json().unwrap()["type"],
            "long_tap"
        );
    }

    #[test]
    fn invalid_interactive_steps_fail_loud() {
        let detector = detector();
        let missing_coordinate = EnvDetectionStep {
            kind: "tap".to_string(),
            x: Some(100),
            y: None,
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: None,
        };
        let err = validate_detection_step(&detector, 0, &missing_coordinate)
            .expect_err("missing coordinate rejected");
        assert!(err.message.contains("missing coordinate y"));

        let invalid_duration = EnvDetectionStep {
            kind: "wait".to_string(),
            x: None,
            y: None,
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: Some(0),
        };
        let err = validate_detection_step(&detector, 1, &invalid_duration)
            .expect_err("zero duration rejected");
        assert!(err.message.contains("duration_ms"));
    }

    #[test]
    fn dry_run_detection_steps_plan_without_device_work() {
        let mut detector = detector();
        detector.steps = vec![EnvDetectionStep {
            kind: "tap".to_string(),
            x: Some(100),
            y: Some(200),
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: None,
        }];
        let global = GlobalOptions {
            dry_run: true,
            ..GlobalOptions::default()
        };
        let flags = FlagArgs::parse(&[]).unwrap();
        let run = run_detection_steps(&global, &flags, &detector).unwrap();
        assert!(run.planned_only);
        assert!(!run.executed);
        assert_eq!(run.steps[0]["step"]["type"], "tap");
    }

    #[test]
    fn stale_resource_hash_blocks_resolution() {
        let temp = TempDir::new().unwrap();
        let context = context(temp.path(), "envinst_a");
        let detector = detector();
        let result = result(&context, &detector, "old-hash", "Default", 0.95, None);
        let err = ensure_result_fresh(&result, &detector, &context, "new-hash", current_unix_ms())
            .expect_err("stale hash rejected");
        assert!(err.message.contains("resource hash changed"));
    }

    #[test]
    fn low_confidence_blocks_resolution() {
        let temp = TempDir::new().unwrap();
        let context = context(temp.path(), "envinst_a");
        let detector = detector();
        let result = result(&context, &detector, "hash", "Default", 0.60, None);
        let err = ensure_result_fresh(&result, &detector, &context, "hash", current_unix_ms())
            .expect_err("low confidence rejected");
        assert!(err.message.contains("confidence"));
    }

    #[test]
    fn env_pointer_resolution_uses_detected_value() {
        let temp = TempDir::new().unwrap();
        let context = context(temp.path(), "envinst_a");
        let detector = detector();
        let result = result(&context, &detector, "hash", "Default", 0.95, None);
        let (resolved, keys) = resolve_env_markers(
            "hometheme/{env:ui_theme}/DepotEnter.png",
            &detector,
            &result,
            current_unix_ms(),
        )
        .unwrap();
        assert_eq!(resolved, "hometheme/Default/DepotEnter.png");
        assert_eq!(keys[0].key, "ui_theme");
        assert_eq!(keys[0].value, "Default");
    }

    #[test]
    fn lock_conflict_is_visible() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("envinst_a/result.json");
        let first = EnvResultLock::acquire(&path).unwrap();
        let err = EnvResultLock::acquire(&path).expect_err("second lock rejected");
        assert!(err.message.contains("lock conflict"));
        first.release().unwrap();
        EnvResultLock::acquire(&path).unwrap().release().unwrap();
    }

    #[test]
    fn detection_writes_result_and_resolution_reads_it() {
        let temp = TempDir::new().unwrap();
        let resource_root = temp.path();
        fs::create_dir_all(resource_root.join("templates")).unwrap();
        fs::write(
            resource_root.join("templates/default.png"),
            encode_png(1, 1, [255, 0, 0]),
        )
        .unwrap();
        let context = context(resource_root, "envinst_a");
        let detector = detector();
        let scene = Scene::from_pixels(1, 1, &[255, 0, 0], ScenePixelFormat::Rgb8).unwrap();
        let hash = detector_resource_hash(&detector, resource_root).unwrap();
        let result =
            evaluate_detector(&detector, &context, &scene, &hash, current_unix_ms()).unwrap();
        let path = env_result_path(&context.env_dir, &context.instance_id);
        write_env_result(&path, &result).unwrap();
        let loaded = load_env_result(&path).unwrap();
        ensure_result_fresh(&loaded, &detector, &context, &hash, current_unix_ms()).unwrap();
        assert_eq!(loaded.detections["ui_theme"].value, "Default");
    }

    fn candidate(value: &str) -> EnvDetectionCandidate {
        EnvDetectionCandidate {
            value: value.to_string(),
            template_path: "templates/default.png".to_string(),
            region: None,
            threshold: None,
            source: None,
        }
    }

    fn detector() -> EnvDetector {
        EnvDetector {
            id: "detect_ui_theme".to_string(),
            version: Some("1".to_string()),
            game_id: Some("arknights".to_string()),
            server_id: Some("cn".to_string()),
            resource_pack_id: Some("test-pack".to_string()),
            match_metric: Some("ccorr_normed".to_string()),
            steps: Vec::new(),
            keys: vec![EnvDetectionKey {
                key: "ui_theme".to_string(),
                min_confidence: 0.7,
                stale_below_confidence: Some(0.7),
                ttl_ms: None,
                allowed_values: vec!["Default".to_string()],
                candidates: vec![candidate("Default")],
            }],
        }
    }

    fn context(root: &Path, instance_id: &str) -> EnvCommandContext {
        EnvCommandContext {
            resource_root: root.to_path_buf(),
            env_dir: root.join(ENV_DETECTION_DIR),
            instance_id: instance_id.to_string(),
            game_id: "arknights".to_string(),
            server_id: "cn".to_string(),
        }
    }

    fn result(
        context: &EnvCommandContext,
        detector: &EnvDetector,
        resource_hash: &str,
        value: &str,
        confidence: f32,
        expires_at_unix_ms: Option<u64>,
    ) -> EnvDetectionResult {
        let now = current_unix_ms();
        EnvDetectionResult {
            schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
            instance_id: context.instance_id.clone(),
            game_id: context.game_id.clone(),
            server_id: context.server_id.clone(),
            detector_id: detector.id.clone(),
            detector_version: detector.version().to_string(),
            resource_pack_id: detector.resource_pack_id(context),
            resource_pack_hash: resource_hash.to_string(),
            generated_at_unix_ms: now,
            detections: BTreeMap::from([(
                "ui_theme".to_string(),
                EnvDetectedValue {
                    value: value.to_string(),
                    confidence,
                    source: "test".to_string(),
                    detected_at_unix_ms: now,
                    detector_id: detector.id.clone(),
                    expires_at_unix_ms,
                },
            )]),
        }
    }

    fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        let pixels = vec![color; usize::try_from(width * height).unwrap()];
        encode_rgb_png(width, height, &pixels)
    }

    fn encode_rgb_png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
        let mut raw = Vec::new();
        for row in 0..height {
            raw.push(0);
            let start = usize::try_from(row * width).unwrap();
            let end = start + usize::try_from(width).unwrap();
            for pixel in &pixels[start..end] {
                raw.extend_from_slice(pixel);
            }
        }
        let mut zlib = vec![0x78, 0x01];
        write_uncompressed_deflate(&mut zlib, &raw);
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        write_chunk(&mut png, b"IHDR", &ihdr);
        write_chunk(&mut png, b"IDAT", &zlib);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
        for (index, chunk) in data.chunks(65_535).enumerate() {
            let is_last = index == data.len().div_ceil(65_535) - 1;
            out.push(u8::from(is_last));
            let len = u16::try_from(chunk.len()).unwrap();
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a = 1u32;
        let mut b = 0u32;
        for byte in data {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
