use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, RUNTIME_VERSION, UserConfig, canonical_game,
    canonical_server, capture_for_command, current_unix_ms, device_config, ensure_path_within,
    hex_sha256, match_metric_name, parse_match_metric_flag, parse_optional_duration_ms,
    parse_optional_unit_f64, parse_point_pair, parse_record_build_resolution,
    parse_record_duration_ms, parse_session_record_candidate_index, parse_session_record_region,
    parse_session_record_swipe_rects, read_json_file, read_user_config, record_amend_step_id,
    record_candidates_step_id, required_non_empty_flag, resolve_instance_id_for_flags,
    resolve_resource_root, resource_authoring, runtime_slice_cli, runtime_state_root,
    safe_file_stem, scene_from_frame, session_record_drift_diagnostics_path,
    session_state_dir_from_flags, write_json_file_atomic,
};
use actingcommand_contract::{EventActor, EventSource};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::{MatchMetric, Rect as RecognitionRect};
use actingcommand_resource_tooling::canonical_locale;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordContext {
    pub(crate) schema_version: String,
    pub(crate) record_id: String,
    pub(crate) task_id: String,
    pub(crate) instance: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) holder: Option<String>,
    #[serde(default)]
    pub(crate) lease_id: Option<String>,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) updated_at_unix_ms: u64,
    #[serde(default)]
    pub(crate) steps: Vec<SessionRecordStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordStep {
    pub(crate) schema_version: String,
    pub(crate) step_id: String,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) updated_at_unix_ms: u64,
    #[serde(flatten)]
    pub(crate) data: SessionRecordStepData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SessionRecordStepData {
    Anchor {
        id: String,
        region: SessionRecordRegion,
        color_check: bool,
        #[serde(default)]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<Box<SessionRecordAnchorArtifact>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    ColorProbe {
        id: String,
        region: SessionRecordRegion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected: Option<[u8; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    VerifyTemplate {
        id: String,
        region: SessionRecordRegion,
        #[serde(default)]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_provenance: Option<Box<SessionRecordFrameProvenance>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact: Option<Box<SessionRecordAnchorArtifact>>,
        evaluation: Box<SessionRecordStepEvaluation>,
    },
    Operation {
        from: String,
        #[serde(default)]
        to: Option<String>,
        click: SessionRecordClick,
        destructive: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum SessionRecordRegion {
    Auto,
    Rect { rect: SessionRecordRect },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SessionRecordClick {
    Coord {
        x: i32,
        y: i32,
    },
    Target {
        target: String,
    },
    Swipe {
        from: SessionRecordRect,
        to: SessionRecordRect,
        duration_ms: u64,
    },
    LongPress {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordStepEvaluation {
    pub(crate) status: String,
    pub(crate) reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auto_region: Option<SessionRecordAutoRegionSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) backtest: Option<SessionRecordAnchorBacktest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) contrast_backtest: Option<SessionRecordAnchorContrastBacktest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordAutoRegionSelection {
    strategy: String,
    selected_reason: String,
    selected: SessionRecordRect,
    candidates: Vec<SessionRecordAutoRegionCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecordAutoRegionCandidate {
    region: SessionRecordRect,
    luma_variance: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contrast_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contrast_passed: Option<bool>,
    selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordAnchorBacktest {
    pub(crate) source: String,
    pub(crate) metric: String,
    pub(crate) region: SessionRecordRect,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) raw_score: f32,
    pub(crate) score: f32,
    pub(crate) threshold: f32,
    pub(crate) passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordAnchorContrastBacktest {
    source: String,
    path: String,
    sha256: String,
    width: u32,
    height: u32,
    metric: String,
    region: SessionRecordRect,
    x: i32,
    y: i32,
    raw_score: f32,
    score: f32,
    threshold: f32,
    passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordFrameProvenance {
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) recorded_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capture_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) freshness: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) capture_attempts: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecordAnchorArtifact {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) region: SessionRecordRect,
}

pub(crate) struct MaterializedAnchorArtifact {
    region: SessionRecordRegion,
    pub(crate) frame_provenance: SessionRecordFrameProvenance,
    pub(crate) artifact: SessionRecordAnchorArtifact,
    pub(crate) evaluation: SessionRecordStepEvaluation,
}

pub(crate) struct SessionRecordAnchorRegionResolution {
    pub(crate) rect: SessionRecordRect,
    pub(crate) auto_region: Option<SessionRecordAutoRegionSelection>,
}

pub(crate) struct SessionRecordSourceFrame {
    pub(crate) frame: Frame,
    pub(crate) png: Vec<u8>,
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) recorded_at_unix_ms: u64,
    pub(crate) capture_backend: Option<String>,
    pub(crate) freshness: Option<Value>,
    pub(crate) capture_attempts: Vec<Value>,
}

struct SessionRecordContrastFrame {
    frame: Frame,
    path: PathBuf,
    sha256: String,
}

struct SessionRecordStepContext<'a> {
    global: &'a GlobalOptions,
    config: &'a UserConfig,
    record: &'a SessionRecordContext,
    state_dir: &'a Path,
}

struct SessionRecordAmendContext {
    record_id: String,
    state_dir: PathBuf,
}

struct SessionRecordAnchorAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    color_check: &'a mut bool,
    threshold: &'a mut Option<f64>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    artifact: &'a mut Option<Box<SessionRecordAnchorArtifact>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

struct SessionRecordColorProbeAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    expected: &'a mut Option<[u8; 3]>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

struct SessionRecordVerifyTemplateAmendTarget<'a> {
    id: &'a mut String,
    region: &'a mut SessionRecordRegion,
    threshold: &'a mut Option<f64>,
    frame_provenance: &'a mut Option<Box<SessionRecordFrameProvenance>>,
    artifact: &'a mut Option<Box<SessionRecordAnchorArtifact>>,
    evaluation: &'a mut SessionRecordStepEvaluation,
}

#[derive(Debug)]
pub(crate) struct SessionRecordDriftDiagnostics {
    path: PathBuf,
    target_id: String,
    pub(crate) region: SessionRecordRect,
    threshold: Option<f64>,
    pub(crate) changed_fields: Vec<&'static str>,
}

pub(crate) struct SessionRecordBuildDraft {
    root: PathBuf,
    task_dir_name: String,
    bundle: Value,
    task_dir: PathBuf,
    task_path: PathBuf,
    resources_path: PathBuf,
    assets: Vec<SessionRecordBuildAsset>,
}

struct SessionRecordBuildAsset {
    source: PathBuf,
    destination: PathBuf,
    template: String,
}

pub(crate) fn run_session_record(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    run_session_record_inner(global, args, None)
}

fn run_session_record_inner(
    global: &GlobalOptions,
    args: &[String],
    forced_state_dir: Option<&Path>,
) -> CliOutcome<Value> {
    let action = args.first().map(String::as_str).ok_or_else(|| {
        CliError::usage(
            "session record requires start|status|stop|step|candidates|amend|build-task|promote",
        )
    })?;
    let flags = FlagArgs::parse(&args[1..])?;
    let config = read_user_config()?;
    let state_dir = forced_state_dir
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(|| session_state_dir_from_flags(&flags))?;
    fs::create_dir_all(&state_dir).map_err(|err| {
        CliError::runtime_not_running(format!(
            "failed to create session state dir {}: {err}",
            state_dir.display()
        ))
    })?;
    let instance_id = resolve_instance_id_for_flags(global, &config, &flags)?;
    let record_path = session_record_path(&state_dir, &instance_id);
    match action {
        "start" => {
            let task_id = flags.required("--task-id")?;
            if task_id.trim().is_empty() {
                return Err(CliError::usage("--task-id must not be empty"));
            }
            if record_path.exists()
                && !flags.bool("--force")
                && let Some(existing) = read_json_file::<SessionRecordContext>(&record_path)?
                && existing.status == "active"
            {
                return Err(CliError::safety_blocked(
                    "record_session_active",
                    format!(
                        "recording session already active for {} with task {}",
                        existing.instance, existing.task_id
                    ),
                    &["session_record"],
                ));
            }
            let record = new_session_record(&instance_id, &task_id, &flags);
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "started",
                "record": record,
                "path": record_path.display().to_string(),
                "auto_recording": false
            }))
        }
        "status" => Ok(json!({
            "status": if record_path.exists() { "available" } else { "not_started" },
            "instance": instance_id,
            "record": read_json_file::<SessionRecordContext>(&record_path)?,
            "path": record_path.display().to_string()
        })),
        "stop" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Ok(json!({
                    "status": "not_started",
                    "instance": instance_id,
                    "path": record_path.display().to_string()
                }));
            };
            record.status = "stopped".to_string();
            record.updated_at_unix_ms = current_unix_ms();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "stopped",
                "record": record,
                "path": record_path.display().to_string()
            }))
        }
        "step" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            if record.status != "active" {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "recording session for {} is {}, not active",
                        instance_id, record.status
                    ),
                    &["session_record"],
                ));
            }
            let step_context = SessionRecordStepContext {
                global,
                config: &config,
                record: &record,
                state_dir: &state_dir,
            };
            let step = new_session_record_step(&step_context, &flags)?;
            record.steps.push(step.clone());
            record.updated_at_unix_ms = current_unix_ms();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "step_recorded",
                "step": step,
                "record": record,
                "path": record_path.display().to_string(),
                "step_count": record.steps.len()
            }))
        }
        "amend" => {
            let Some(mut record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            if record.status != "active" {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "recording session for {} is {}, not active",
                        instance_id, record.status
                    ),
                    &["session_record"],
                ));
            }
            let amend_context = SessionRecordAmendContext {
                record_id: record.record_id.clone(),
                state_dir: state_dir.clone(),
            };
            if let Some(diagnostics_path) = session_record_drift_diagnostics_path(&flags)? {
                let amend = amend_session_record_from_drift_diagnostics(
                    &amend_context,
                    &mut record,
                    &flags,
                    diagnostics_path,
                )?;
                record.updated_at_unix_ms = current_unix_ms();
                write_json_file_atomic(&record_path, &record)?;
                return Ok(json!({
                    "status": "drift_diagnostics_amended",
                    "amend": amend,
                    "record": record,
                    "path": record_path.display().to_string(),
                    "step_count": record.steps.len()
                }));
            }
            let step_id = record_amend_step_id(&flags)?;
            let Some(step) = record.steps.iter_mut().find(|step| step.step_id == step_id) else {
                return Err(CliError::safety_blocked(
                    "record_step_not_found",
                    format!("recording step does not exist: {step_id}"),
                    &["session_record"],
                ));
            };
            amend_session_record_step(&amend_context, step, &flags)?;
            record.updated_at_unix_ms = current_unix_ms();
            let amended_step = step.clone();
            write_json_file_atomic(&record_path, &record)?;
            Ok(json!({
                "status": "step_amended",
                "step": amended_step,
                "record": record,
                "path": record_path.display().to_string(),
                "step_count": record.steps.len()
            }))
        }
        "candidates" | "candidate-list" => {
            let Some(record) = read_json_file::<SessionRecordContext>(&record_path)? else {
                return Err(CliError::safety_blocked(
                    "record_session_not_active",
                    format!(
                        "no recording session exists for {}; run session record start first",
                        instance_id
                    ),
                    &["session_record"],
                ));
            };
            let step_id = record_candidates_step_id(&flags)?;
            let Some(step) = record.steps.iter().find(|step| step.step_id == step_id) else {
                return Err(CliError::safety_blocked(
                    "record_step_not_found",
                    format!("recording step does not exist: {step_id}"),
                    &["session_record"],
                ));
            };
            session_record_candidate_report(&record, step, &record_path)
        }
        "build-task" => {
            build_session_record_task(global, &config, &flags, &record_path, &instance_id)
        }
        "promote" | "publish" => {
            promote_session_record_task(global, &config, &flags, &record_path, &instance_id)
        }
        other => Err(CliError::usage(format!(
            "unknown session record action: {other}"
        ))),
    }
}

fn build_session_record_task(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    record_path: &Path,
    instance_id: &str,
) -> CliOutcome<Value> {
    let Some(record) = read_json_file::<SessionRecordContext>(record_path)? else {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "no recording session exists for {instance_id}; run session record start first"
            ),
            &["session_record"],
        ));
    };
    if !matches!(record.status.as_str(), "active" | "stopped") {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "recording session for {} is {}, not active or stopped",
                record.instance, record.status
            ),
            &["session_record"],
        ));
    }
    let out = flags.required_path("--out")?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let (game, server, locale) = session_record_selector(global, config, flags, instance_id)?;
    let state_dir = record_path.parent().unwrap_or_else(|| Path::new("."));
    let draft =
        session_record_build_draft(&record, flags, &out, &game, &server, &locale, state_dir)?;
    let authoring = session_record_authoring_input(&record, &draft)?;
    if !dry_run {
        resource_authoring::materialize_record_authoring(&out, &authoring)?;
    }
    Ok(json!({
        "status": if dry_run { "validated" } else { "built" },
        "mode": "session-record-build-task",
        "dry_run": dry_run,
        "instance": instance_id,
        "record_id": record.record_id,
        "task_id": record.task_id,
        "game": game,
        "server": server,
        "locale": locale,
        "out": out.display().to_string(),
        "task_dir": draft.task_dir.display().to_string(),
        "task_path": draft.task_path.display().to_string(),
        "resources_path": draft.resources_path.display().to_string(),
        "anchor_count": draft.bundle.get("anchors").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "color_probe_count": draft.bundle.get("color_probes").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "verify_template_count": draft.bundle.get("verify_templates").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "operation_count": draft.bundle.get("operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "asset_count": draft.assets.len(),
        "assets": draft.assets.iter().map(|asset| {
            json!({
                "template": &asset.template,
                "source": asset.source.display().to_string(),
                "destination": asset.destination.display().to_string()
            })
        }).collect::<Vec<_>>(),
        "bundle": draft.bundle
    }))
}

fn promote_session_record_task(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    record_path: &Path,
    instance_id: &str,
) -> CliOutcome<Value> {
    let Some(record) = read_json_file::<SessionRecordContext>(record_path)? else {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "no recording session exists for {instance_id}; run session record start first"
            ),
            &["session_record"],
        ));
    };
    if !matches!(record.status.as_str(), "active" | "stopped") {
        return Err(CliError::safety_blocked(
            "record_session_not_active",
            format!(
                "recording session for {} is {}, not active or stopped",
                record.instance, record.status
            ),
            &["session_record"],
        ));
    }
    let repo = flags.required_path("--repo")?;
    let resource_root = resolve_resource_root(&repo);
    if resource_root.layout == "unresolved" {
        return Err(CliError::usage(
            "session record promote requires --repo to be an existing resource root or a repository containing ours/",
        ));
    }
    let dry_run = global.dry_run || flags.bool("--dry-run");
    let force = flags.bool("--force");
    let (game, server, locale) = session_record_selector(global, config, flags, instance_id)?;
    let state_dir = record_path.parent().unwrap_or_else(|| Path::new("."));
    let draft = session_record_build_draft(
        &record,
        flags,
        &resource_root.root,
        &game,
        &server,
        &locale,
        state_dir,
    )?;
    let authoring_input = session_record_authoring_input(&record, &draft)?;
    if draft.task_dir.exists() && !force {
        return Err(CliError::safety_blocked(
            "record_promote_target_exists",
            format!(
                "record promote target task directory already exists: {}; use --force to replace it",
                draft.task_dir.display()
            ),
            &["session_record", "resource_repo"],
        ));
    }
    let resources_existed = draft.resources_path.exists();
    let (resources_action, authoring) = if dry_run {
        let action = if resources_existed {
            "would_preserve"
        } else {
            "would_create"
        };
        (action, Value::Null)
    } else {
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            runtime_state_root()?,
            EventActor::Lab,
            EventSource::Lab,
        ))
        .map_err(runtime_slice_cli::map_runtime_error)?;
        let target_label = format!(
            "{}-{}-resources",
            safe_file_stem(&game),
            safe_file_stem(&server)
        );
        let output = resource_authoring::publish_record_authoring(
            &client,
            &resource_root.root,
            target_label,
            &authoring_input,
            &game,
            &server,
            force,
        )?;
        let output = serde_json::to_value(output).map_err(|error| {
            CliError::usage(format!(
                "failed to serialize resource authoring receipt: {error}"
            ))
        })?;
        (
            if resources_existed {
                "preserved"
            } else {
                "created"
            },
            output,
        )
    };
    Ok(json!({
        "status": if dry_run { "validated" } else { "promoted" },
        "mode": "session-record-promote",
        "dry_run": dry_run,
        "force": force,
        "instance": instance_id,
        "record_id": record.record_id,
        "task_id": record.task_id,
        "game": game,
        "server": server,
        "locale": locale,
        "repo": resource_root.input.display().to_string(),
        "resource_root": resource_root.root.display().to_string(),
        "resource_layout": resource_root.layout,
        "task_dir": draft.task_dir.display().to_string(),
        "task_path": draft.task_path.display().to_string(),
        "resources_path": draft.resources_path.display().to_string(),
        "resources_action": resources_action,
        "authoring": authoring,
        "anchor_count": draft.bundle.get("anchors").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "color_probe_count": draft.bundle.get("color_probes").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "verify_template_count": draft.bundle.get("verify_templates").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "operation_count": draft.bundle.get("operations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "asset_count": draft.assets.len(),
        "assets": draft.assets.iter().map(|asset| {
            json!({
                "template": &asset.template,
                "source": asset.source.display().to_string(),
                "destination": asset.destination.display().to_string()
            })
        }).collect::<Vec<_>>()
    }))
}

pub(crate) fn session_record_build_draft(
    record: &SessionRecordContext,
    flags: &FlagArgs,
    out: &Path,
    game: &str,
    server: &str,
    locale: &str,
    state_dir: &Path,
) -> CliOutcome<SessionRecordBuildDraft> {
    let task_dir_name = safe_task_dir_name(&record.task_id)?;
    let task_dir = out.join("operations").join(&task_dir_name);
    let resources_path = out.join("operations").join("resources.json");
    let task_path = task_dir.join("task.json");
    let assets_dir = task_dir.join("assets");
    let mut assets = Vec::new();
    let mut anchors = Vec::new();
    let mut anchor_templates = BTreeMap::new();
    let mut resolution = parse_record_build_resolution(flags)?;

    for step in &record.steps {
        if let SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } = &step.data
        {
            let artifact = artifact.as_deref().ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build anchor '{}' without a frame artifact",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires anchor '{}' to pass backtest; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            if resolution.is_none()
                && let Some(provenance) = frame_provenance.as_deref()
            {
                resolution = Some((provenance.width, provenance.height));
            }
            let source = ensure_path_within(
                state_dir,
                Path::new(&artifact.path),
                "record build-task artifact source",
                &["record", "artifact_path"],
            )?;
            if !source.is_file() {
                return Err(CliError::usage(format!(
                    "record build-task anchor '{}' artifact is missing: {}",
                    step.step_id,
                    source.display()
                )));
            }
            let color_check_value = session_record_bundle_color_check(
                *color_check,
                frame_provenance.as_deref(),
                &artifact.region,
                &step.step_id,
            )?;
            let asset_name = format!(
                "anchor-{}-{}.png",
                safe_file_stem(&step.step_id),
                safe_file_stem(id)
            );
            let destination = assets_dir.join(&asset_name);
            let template = format!("assets/{asset_name}");
            assets.push(SessionRecordBuildAsset {
                source,
                destination,
                template: template.clone(),
            });
            anchor_templates.insert(id.clone(), template.clone());
            anchors.push(json!({
                "id": id,
                "template": template,
                "region": region,
                "threshold": threshold.unwrap_or_else(|| {
                    evaluation
                        .backtest
                        .as_ref()
                        .map(|backtest| f64::from(backtest.threshold))
                        .unwrap_or(0.95)
                }),
                "color_check": color_check_value,
                "provenance": {
                    "record_step_id": step.step_id,
                    "record_color_check_requested": color_check,
                    "frame_provenance": frame_provenance,
                    "artifact": artifact,
                    "evaluation": evaluation
                }
            }));
        }
    }

    let mut color_probes = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::ColorProbe {
            id,
            region,
            expected,
            frame_provenance,
            evaluation,
        } = &step.data
        {
            let expected = expected.ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build color-probe '{}' without expected color; provide --frame or --capture when recording it",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires color-probe '{}' to pass evaluation; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            color_probes.push(json!({
                "id": id,
                "region": region,
                "expected": expected,
                "provenance": {
                    "record_step_id": step.step_id,
                    "frame_provenance": frame_provenance,
                    "evaluation": evaluation,
                    "created_at_unix_ms": step.created_at_unix_ms,
                    "updated_at_unix_ms": step.updated_at_unix_ms
                }
            }));
        }
    }

    let mut verify_templates = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } = &step.data
        {
            let artifact = artifact.as_deref().ok_or_else(|| {
                CliError::usage(format!(
                    "record build-task cannot build verify-template '{}' without a frame artifact",
                    step.step_id
                ))
            })?;
            if evaluation.status != "passed" {
                return Err(CliError::usage(format!(
                    "record build-task requires verify-template '{}' to pass backtest; status is {}",
                    step.step_id, evaluation.status
                )));
            }
            if resolution.is_none()
                && let Some(provenance) = frame_provenance.as_deref()
            {
                resolution = Some((provenance.width, provenance.height));
            }
            let source = ensure_path_within(
                state_dir,
                Path::new(&artifact.path),
                "record build-task artifact source",
                &["record", "artifact_path"],
            )?;
            if !source.is_file() {
                return Err(CliError::usage(format!(
                    "record build-task verify-template '{}' artifact is missing: {}",
                    step.step_id,
                    source.display()
                )));
            }
            let asset_name = format!(
                "verify-template-{}-{}.png",
                safe_file_stem(&step.step_id),
                safe_file_stem(id)
            );
            let destination = assets_dir.join(&asset_name);
            let template = format!("assets/{asset_name}");
            assets.push(SessionRecordBuildAsset {
                source,
                destination,
                template: template.clone(),
            });
            verify_templates.push(json!({
                "id": id,
                "template": template,
                "region": region,
                "threshold": threshold.unwrap_or_else(|| {
                    evaluation
                        .backtest
                        .as_ref()
                        .map(|backtest| f64::from(backtest.threshold))
                        .unwrap_or(0.95)
                }),
                "provenance": {
                    "record_step_id": step.step_id,
                    "frame_provenance": frame_provenance,
                    "artifact": artifact,
                    "evaluation": evaluation
                }
            }));
        }
    }

    let mut operations = Vec::new();
    for step in &record.steps {
        if let SessionRecordStepData::Operation {
            from,
            to,
            click,
            destructive,
        } = &step.data
        {
            let click_value = session_record_bundle_click(click, &step.step_id)?;
            validate_record_build_page_ref("from", from, &anchor_templates, &step.step_id)?;
            if let Some(to) = to {
                validate_record_build_page_ref("to", to, &anchor_templates, &step.step_id)?;
            }
            let verify_template = to.as_ref().and_then(|to| anchor_templates.get(to)).cloned();
            let guard = session_record_operation_guard(from, click, &anchor_templates)?;
            operations.push(json!({
                "id": step.step_id,
                "purpose": format!("recorded operation from {from}"),
                "from": from,
                "to": to,
                "click": click_value,
                "verify_template": verify_template,
                "guard": guard,
                "consumes": [],
                "produces": [],
                "destructive": destructive,
                "provenance": {
                    "record_step_id": step.step_id,
                    "created_at_unix_ms": step.created_at_unix_ms,
                    "updated_at_unix_ms": step.updated_at_unix_ms
                }
            }));
        }
    }
    if operations.is_empty() {
        return Err(CliError::usage(
            "record build-task requires at least one operation step",
        ));
    }
    let (width, height) = resolution.ok_or_else(|| {
        CliError::usage("record build-task requires --resolution <width>x<height> when no frame-backed anchor is available")
    })?;
    validate_record_build_operation_clicks(&operations, width, height)?;
    let entry_page = flags
        .optional("--entry-page")
        .filter(|value| value != "true")
        .or_else(|| {
            operations
                .first()
                .and_then(|operation| operation.get("from"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let target_page = flags
        .optional("--target-page")
        .filter(|value| value != "true")
        .or_else(|| {
            operations
                .iter()
                .rev()
                .find_map(|operation| operation.get("to").and_then(Value::as_str))
                .map(str::to_string)
        });
    if let Some(entry_page) = &entry_page {
        validate_record_build_page_ref(
            "entry_page",
            entry_page,
            &anchor_templates,
            &record.task_id,
        )?;
    }
    if let Some(target_page) = &target_page {
        validate_record_build_page_ref(
            "target_page",
            target_page,
            &anchor_templates,
            &record.task_id,
        )?;
    }
    let recorded_at_unix_ms = record
        .steps
        .iter()
        .map(|step| step.created_at_unix_ms)
        .min()
        .unwrap_or(record.started_at_unix_ms);
    let bundle = json!({
        "schema_version": "0.5",
        "task_id": record.task_id,
        "game": game,
        "server_scope": [server],
        "locale": locale,
        "goal": flags
            .optional("--goal")
            .filter(|value| value != "true")
            .unwrap_or_else(|| format!("recorded from {}", record.record_id)),
        "coordinate_space": {"width": width, "height": height},
        "defaults": {
            "template_threshold": parse_optional_unit_f64(flags, "--default-threshold")?.unwrap_or(0.95),
            "color_max_distance": 20.0,
            "match_metric": flags
                .optional("--metric")
                .filter(|value| value != "true")
                .unwrap_or_else(|| "ccorr_normed".to_string())
        },
        "anchors": anchors,
        "color_probes": color_probes,
        "verify_templates": verify_templates,
        "entry_page": entry_page,
        "target_page": target_page,
        "operations": operations,
        "provenance": {
            "source": "session_record",
            "game": game,
            "server": server,
            "locale": locale,
            "resolution": {"width": width, "height": height},
            "recorded_at_unix_ms": recorded_at_unix_ms,
            "runtime_version": RUNTIME_VERSION,
            "client_version": flags.optional("--client-version").filter(|value| value != "true"),
            "record_id": record.record_id,
            "record_status": record.status,
            "instance": record.instance,
            "holder": record.holder,
            "lease_id": record.lease_id,
            "started_at_unix_ms": record.started_at_unix_ms,
            "updated_at_unix_ms": record.updated_at_unix_ms
        }
    });
    Ok(SessionRecordBuildDraft {
        root: out.to_path_buf(),
        task_dir_name,
        bundle,
        task_dir,
        task_path,
        resources_path,
        assets,
    })
}

fn session_record_authoring_input(
    record: &SessionRecordContext,
    draft: &SessionRecordBuildDraft,
) -> CliOutcome<resource_authoring::RecordAuthoringInput> {
    let assets = draft
        .assets
        .iter()
        .map(|asset| {
            let relative_path = asset.destination.strip_prefix(&draft.root).map_err(|_| {
                CliError::safety_blocked(
                    "authoring_asset_path_escape",
                    format!(
                        "record authoring asset {} is outside target root {}",
                        asset.destination.display(),
                        draft.root.display()
                    ),
                    &["session_record", "resource_authoring"],
                )
            })?;
            Ok(resource_authoring::RecordAuthoringAsset {
                source: asset.source.clone(),
                relative_path: relative_path.to_path_buf(),
            })
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    Ok(resource_authoring::RecordAuthoringInput {
        record_id: record.record_id.clone(),
        task_id: record.task_id.clone(),
        task_dir_name: draft.task_dir_name.clone(),
        bundle: draft.bundle.clone(),
        assets,
    })
}

fn session_record_bundle_color_check(
    enabled: bool,
    frame_provenance: Option<&SessionRecordFrameProvenance>,
    rect: &SessionRecordRect,
    step_id: &str,
) -> CliOutcome<Value> {
    if !enabled {
        return Ok(Value::Null);
    }
    let Some(frame_provenance) = frame_provenance else {
        return Err(CliError::usage(format!(
            "record build-task anchor '{step_id}' requested color_check but has no frame provenance"
        )));
    };
    let source_frame = read_session_record_source_frame_from_provenance(frame_provenance)?;
    let expected = mean_session_record_rect_rgb(&source_frame.frame, rect)?;
    Ok(json!({
        "region": {
            "mode": "rect",
            "rect": rect
        },
        "expected": expected
    }))
}

fn mean_session_record_rect_rgb(frame: &Frame, rect: &SessionRecordRect) -> CliOutcome<[u8; 3]> {
    let crop = crop_frame_rect(frame, rect)?;
    let stride = match crop.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let mut sum = [0_u64; 3];
    for pixel in crop.pixels.chunks_exact(stride) {
        sum[0] += u64::from(pixel[0]);
        sum[1] += u64::from(pixel[1]);
        sum[2] += u64::from(pixel[2]);
    }
    let count = u64::from(crop.width)
        .checked_mul(u64::from(crop.height))
        .ok_or_else(|| CliError::usage("record color_check pixel count overflow"))?;
    if count == 0 {
        return Err(CliError::usage("record color_check region has no pixels"));
    }
    Ok([
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ])
}

fn session_record_bundle_click(click: &SessionRecordClick, step_id: &str) -> CliOutcome<Value> {
    match click {
        SessionRecordClick::Coord { x, y } => Ok(json!({
            "kind": "point",
            "x": x,
            "y": y
        })),
        SessionRecordClick::Swipe {
            from,
            to,
            duration_ms,
        } => Ok(json!({
            "kind": "drag",
            "from": from,
            "to": to,
            "duration_ms": duration_ms
        })),
        SessionRecordClick::LongPress { x, y, duration_ms } => Ok(json!({
            "kind": "long_press",
            "x": x,
            "y": y,
            "duration_ms": duration_ms
        })),
        SessionRecordClick::Target { target } => Err(CliError::usage(format!(
            "record build-task cannot build operation '{step_id}' with unresolved target click '{target}'"
        ))),
    }
}

fn session_record_operation_guard(
    from: &str,
    click: &SessionRecordClick,
    anchors: &BTreeMap<String, String>,
) -> CliOutcome<Value> {
    let (anchor_id, template) = resolve_record_guard_anchor(from, anchors)?;
    Ok(json!({
        "page_id": from,
        "target_id": session_record_anchor_target_id(&anchor_id),
        "expected_rect": session_record_click_expected_rect(click)?,
        "verify_template": template
    }))
}

fn resolve_record_guard_anchor(
    page: &str,
    anchors: &BTreeMap<String, String>,
) -> CliOutcome<(String, String)> {
    if page == "any" {
        return Err(CliError::usage(
            "record build-task cannot build a guarded coordinate operation from page 'any'",
        ));
    }
    if let Some(template) = anchors.get(page) {
        return Ok((page.to_string(), template.clone()));
    }
    let prefix = format!("{page}_");
    anchors
        .iter()
        .find(|(anchor_id, _)| anchor_id.starts_with(&prefix))
        .map(|(anchor_id, template)| (anchor_id.clone(), template.clone()))
        .ok_or_else(|| {
            CliError::usage(format!(
                "record build-task cannot build guard for page '{page}' without a matching anchor"
            ))
        })
}

fn session_record_anchor_target_id(anchor_id: &str) -> String {
    format!("page/{anchor_id}")
}

fn session_record_click_expected_rect(click: &SessionRecordClick) -> CliOutcome<Value> {
    match click {
        SessionRecordClick::Coord { x, y } => Ok(json!({
            "x": x,
            "y": y,
            "width": 1,
            "height": 1
        })),
        SessionRecordClick::Swipe { from, .. } => Ok(json!(from)),
        SessionRecordClick::LongPress { x, y, .. } => Ok(json!({
            "x": x,
            "y": y,
            "width": 1,
            "height": 1
        })),
        SessionRecordClick::Target { target } => Err(CliError::usage(format!(
            "record build-task cannot build a guard for unresolved target click '{target}'"
        ))),
    }
}

fn validate_record_build_operation_clicks(
    operations: &[Value],
    width: u32,
    height: u32,
) -> CliOutcome<()> {
    for operation in operations {
        let operation_id = operation
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let Some(click) = operation.get("click").and_then(Value::as_object) else {
            return Err(CliError::usage(format!(
                "record build-task operation '{operation_id}' is missing click object"
            )));
        };
        match click.get("kind").and_then(Value::as_str) {
            Some("point") | Some("long_press") => {
                let x = click.get("x").and_then(Value::as_i64).ok_or_else(|| {
                    CliError::usage(format!(
                        "record build-task operation '{operation_id}' click.x is missing or not an integer"
                    ))
                })?;
                let y = click.get("y").and_then(Value::as_i64).ok_or_else(|| {
                    CliError::usage(format!(
                        "record build-task operation '{operation_id}' click.y is missing or not an integer"
                    ))
                })?;
                validate_record_build_point(operation_id, x, y, width, height)?;
            }
            Some("drag") => {
                for key in ["from", "to"] {
                    let rect = click.get(key).and_then(Value::as_object).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key} is missing"
                        ))
                    })?;
                    let x = rect.get("x").and_then(Value::as_i64).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key}.x is missing"
                        ))
                    })?;
                    let y = rect.get("y").and_then(Value::as_i64).ok_or_else(|| {
                        CliError::usage(format!(
                            "record build-task operation '{operation_id}' drag.{key}.y is missing"
                        ))
                    })?;
                    validate_record_build_point(operation_id, x, y, width, height)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_record_build_point(
    operation_id: &str,
    x: i64,
    y: i64,
    width: u32,
    height: u32,
) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
        return Err(CliError::usage(format!(
            "record build-task operation '{operation_id}' click point {x},{y} is outside coordinate_space {width}x{height}"
        )));
    }
    Ok(())
}

fn validate_record_build_page_ref(
    label: &str,
    page: &str,
    anchors: &BTreeMap<String, String>,
    owner_id: &str,
) -> CliOutcome<()> {
    if page == "any" {
        return Ok(());
    }
    if anchors.contains_key(page) {
        return Ok(());
    }
    let prefix = format!("{page}_");
    if anchors
        .keys()
        .any(|anchor_id| anchor_id.starts_with(&prefix))
    {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "record build-task {label} page '{page}' in '{owner_id}' has no matching anchor"
    )))
}

fn session_record_selector(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    instance_id: &str,
) -> CliOutcome<(String, String, String)> {
    let instance = config.instances.get(instance_id);
    let game = flags
        .optional("--game")
        .filter(|value| value != "true")
        .or_else(|| global.game.clone())
        .or_else(|| instance.and_then(|instance| instance.game.clone()))
        .ok_or_else(|| {
            CliError::usage("record build-task requires --game or configured instance.<id>.game")
        })?;
    let game = canonical_game(&game)?;
    let server = flags
        .optional("--server")
        .filter(|value| value != "true")
        .or_else(|| global.server.clone())
        .or_else(|| instance.and_then(|instance| instance.server.clone()))
        .ok_or_else(|| {
            CliError::usage(
                "record build-task requires --server or configured instance.<id>.server",
            )
        })?;
    let server = canonical_server(&server)?;
    let locale = flags
        .optional("--locale")
        .filter(|value| value != "true")
        .ok_or_else(|| CliError::usage("record build-task requires --locale"))?;
    let locale = canonical_locale(&locale)?;
    Ok((game, server, locale))
}

fn safe_task_dir_name(task_id: &str) -> CliOutcome<String> {
    let safe = safe_file_stem(task_id);
    if safe != task_id || safe.is_empty() {
        return Err(CliError::usage(format!(
            "record build-task task id must be a safe path segment: {task_id}"
        )));
    }
    Ok(safe)
}

fn new_session_record_step(
    context: &SessionRecordStepContext<'_>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStep> {
    let kind = flags.required("--kind")?;
    let step_id = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .unwrap_or_else(|| format!("step-{:04}", context.record.steps.len() + 1));
    if step_id.trim().is_empty() {
        return Err(CliError::usage("--step-id must not be empty"));
    }
    if context
        .record
        .steps
        .iter()
        .any(|step| step.step_id == step_id)
    {
        return Err(CliError::safety_blocked(
            "record_step_id_conflict",
            format!("recording step id already exists: {step_id}"),
            &["session_record"],
        ));
    }
    let data = match kind.as_str() {
        "anchor" => new_session_record_anchor_step(context, &step_id, flags)?,
        "color-probe" | "color_probe" => {
            new_session_record_color_probe_step(context, &step_id, flags)?
        }
        "verify-template" | "verify_template" => {
            new_session_record_verify_template_step(context, &step_id, flags)?
        }
        "operation" => new_session_record_operation_step(flags)?,
        other => {
            return Err(CliError::usage(format!(
                "unsupported record step kind: {other}"
            )));
        }
    };
    Ok(SessionRecordStep {
        schema_version: "session-record-step-v0".to_string(),
        step_id,
        created_at_unix_ms: current_unix_ms(),
        updated_at_unix_ms: current_unix_ms(),
        data,
    })
}

fn new_session_record_anchor_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let threshold = parse_optional_unit_f64(flags, "--threshold")?;
    let materialized =
        materialize_anchor_artifact(context, step_id, &id, &region, threshold, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::Anchor {
        id,
        region: stored_region,
        color_check: flags.bool("--color-check"),
        threshold,
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        artifact: materialized.map(|materialized| Box::new(materialized.artifact)),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_verify_template_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let threshold = parse_optional_unit_f64(flags, "--threshold")?;
    let materialized =
        materialize_anchor_artifact(context, step_id, &id, &region, threshold, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::VerifyTemplate {
        id,
        region: stored_region,
        threshold,
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        artifact: materialized.map(|materialized| Box::new(materialized.artifact)),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_color_probe_step(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepData> {
    let id = required_non_empty_flag(flags, "--id")?;
    let region = parse_session_record_region(&flags.required("--region")?)?;
    let materialized = materialize_color_probe(context, step_id, &id, &region, flags)?;
    let evaluation = materialized
        .as_ref()
        .map(|materialized| materialized.evaluation.clone())
        .unwrap_or_else(|| SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "frame_not_provided".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        });
    let stored_region = materialized
        .as_ref()
        .map(|materialized| materialized.region.clone())
        .unwrap_or(region);
    Ok(SessionRecordStepData::ColorProbe {
        id,
        region: stored_region,
        expected: materialized
            .as_ref()
            .map(|materialized| materialized.expected),
        frame_provenance: materialized
            .as_ref()
            .map(|materialized| Box::new(materialized.frame_provenance.clone())),
        evaluation: Box::new(evaluation),
    })
}

fn new_session_record_operation_step(flags: &FlagArgs) -> CliOutcome<SessionRecordStepData> {
    let from = required_non_empty_flag(flags, "--from")?;
    let to = flags
        .optional("--to")
        .filter(|value| value != "true")
        .unwrap_or_else(|| "null".to_string());
    Ok(SessionRecordStepData::Operation {
        from,
        to: if to == "null" { None } else { Some(to) },
        click: parse_session_record_operation_click(flags)?,
        destructive: flags.bool("--destructive"),
    })
}

fn materialize_anchor_artifact(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    anchor_id: &str,
    region: &SessionRecordRegion,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<Option<MaterializedAnchorArtifact>> {
    let local_frame_path = flags
        .optional_path("--frame")
        .or_else(|| flags.optional_path("--source-frame"));
    let capture_current_frame = flags.bool("--capture") || flags.bool("--current-frame");
    if local_frame_path.is_some() && capture_current_frame {
        return Err(CliError::usage(
            "record anchor requires either --frame/--source-frame or --capture, not both",
        ));
    }
    if local_frame_path.is_none() && !capture_current_frame {
        return Ok(None);
    }
    let artifact_dir = session_record_artifact_dir(
        context.state_dir,
        &context.record.record_id,
        flags.optional_path("--artifact-dir").as_deref(),
    )?;
    let source_frame = if capture_current_frame {
        capture_session_record_source_frame(
            context.global,
            context.config,
            flags,
            &artifact_dir,
            step_id,
            anchor_id,
        )?
    } else {
        let frame_path = local_frame_path.expect("checked local frame path");
        read_session_record_source_frame(&frame_path)?
    };
    let resolution =
        resolve_session_record_anchor_rect(&source_frame.frame, region, threshold, flags)?;
    materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        anchor_id,
        threshold,
        flags,
    )
    .map(Some)
}

struct MaterializedColorProbe {
    region: SessionRecordRegion,
    expected: [u8; 3],
    frame_provenance: SessionRecordFrameProvenance,
    evaluation: SessionRecordStepEvaluation,
}

fn materialize_color_probe(
    context: &SessionRecordStepContext<'_>,
    step_id: &str,
    probe_id: &str,
    region: &SessionRecordRegion,
    flags: &FlagArgs,
) -> CliOutcome<Option<MaterializedColorProbe>> {
    let local_frame_path = flags
        .optional_path("--frame")
        .or_else(|| flags.optional_path("--source-frame"));
    let capture_current_frame = flags.bool("--capture") || flags.bool("--current-frame");
    if local_frame_path.is_some() && capture_current_frame {
        return Err(CliError::usage(
            "record color-probe requires either --frame/--source-frame or --capture, not both",
        ));
    }
    if local_frame_path.is_none() && !capture_current_frame {
        return Ok(None);
    }
    let artifact_dir = session_record_artifact_dir(
        context.state_dir,
        &context.record.record_id,
        flags.optional_path("--artifact-dir").as_deref(),
    )?;
    let source_frame = if capture_current_frame {
        capture_session_record_source_frame(
            context.global,
            context.config,
            flags,
            &artifact_dir,
            step_id,
            probe_id,
        )?
    } else {
        let frame_path = local_frame_path.expect("checked local frame path");
        read_session_record_source_frame(&frame_path)?
    };
    let resolution = resolve_session_record_anchor_rect(&source_frame.frame, region, None, flags)?;
    let expected = mean_session_record_rect_rgb(&source_frame.frame, &resolution.rect)?;
    Ok(Some(MaterializedColorProbe {
        region: SessionRecordRegion::Rect {
            rect: resolution.rect.clone(),
        },
        expected,
        frame_provenance: session_record_frame_provenance(source_frame),
        evaluation: SessionRecordStepEvaluation {
            status: "passed".to_string(),
            reason: "color_probe_sampled".to_string(),
            auto_region: resolution.auto_region,
            backtest: None,
            contrast_backtest: None,
        },
    }))
}

fn read_session_record_source_frame(frame_path: &Path) -> CliOutcome<SessionRecordSourceFrame> {
    let frame_png = fs::read(frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record source frame {}: {err}",
            frame_path.display()
        ))
    })?;
    let frame = Frame::from_png(frame_png.clone(), CaptureBackendName::AdbScreencap)
        .map_err(|err| CliError::usage(format!("failed to decode record source frame: {err}")))?;
    Ok(SessionRecordSourceFrame {
        frame,
        png: frame_png,
        source: "local_png".to_string(),
        path: frame_path.to_path_buf(),
        recorded_at_unix_ms: current_unix_ms(),
        capture_backend: None,
        freshness: None,
        capture_attempts: Vec::new(),
    })
}

fn session_record_artifact_root(state_dir: &Path, record_id: &str) -> PathBuf {
    state_dir
        .join("record-artifacts")
        .join(safe_file_stem(record_id))
}

fn session_record_artifact_dir(
    state_dir: &Path,
    record_id: &str,
    requested: Option<&Path>,
) -> CliOutcome<PathBuf> {
    let default_dir = session_record_artifact_root(state_dir, record_id);
    let candidate = requested.unwrap_or(default_dir.as_path());
    let resolved = ensure_path_within(
        state_dir,
        candidate,
        "record artifact directory",
        &["record", "artifact_dir"],
    )?;
    fs::create_dir_all(&resolved).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            resolved.display()
        ))
    })?;
    ensure_path_within(
        state_dir,
        &resolved,
        "record artifact directory",
        &["record", "artifact_dir"],
    )
}

fn capture_session_record_source_frame(
    global: &GlobalOptions,
    config: &UserConfig,
    flags: &FlagArgs,
    artifact_dir: &Path,
    step_id: &str,
    anchor_id: &str,
) -> CliOutcome<SessionRecordSourceFrame> {
    let device_config = device_config(global, config)?;
    let requested = device_config.capture_backend;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let captured = capture_for_command(
        &device_config,
        requested,
        flags.bool("--require-fresh"),
        fresh_delay,
    )?;
    let png = captured.frame.png_for_artifact().map_err(|err| {
        CliError::device(format!("failed to encode record source capture: {err}"))
    })?;
    fs::create_dir_all(artifact_dir).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            artifact_dir.display()
        ))
    })?;
    let source_path = artifact_dir.join(format!(
        "source-frame-{}-{}.png",
        safe_file_stem(step_id),
        safe_file_stem(anchor_id)
    ));
    fs::write(&source_path, &png).map_err(|err| {
        CliError::usage(format!(
            "failed to write record source frame {}: {err}",
            source_path.display()
        ))
    })?;
    Ok(SessionRecordSourceFrame {
        capture_backend: Some(captured.frame.backend_name.as_str().to_string()),
        freshness: Some(captured.freshness),
        capture_attempts: captured.attempts,
        frame: captured.frame,
        png,
        source: "current_capture".to_string(),
        path: source_path,
        recorded_at_unix_ms: current_unix_ms(),
    })
}

fn session_record_frame_provenance(
    source_frame: SessionRecordSourceFrame,
) -> SessionRecordFrameProvenance {
    SessionRecordFrameProvenance {
        source: source_frame.source,
        path: source_frame.path.display().to_string(),
        sha256: hex_sha256(&source_frame.png),
        width: source_frame.frame.width,
        height: source_frame.frame.height,
        recorded_at_unix_ms: source_frame.recorded_at_unix_ms,
        capture_backend: source_frame.capture_backend,
        freshness: source_frame.freshness,
        capture_attempts: source_frame.capture_attempts,
    }
}

pub(crate) fn materialize_anchor_artifact_from_source(
    source_frame: SessionRecordSourceFrame,
    resolution: SessionRecordAnchorRegionResolution,
    artifact_dir: &Path,
    step_id: &str,
    anchor_id: &str,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<MaterializedAnchorArtifact> {
    let rect = &resolution.rect;
    let crop = crop_frame_rect(&source_frame.frame, rect)?;
    let crop_png = crop
        .png_for_artifact()
        .map_err(|err| CliError::usage(format!("failed to encode record anchor crop: {err}")))?;
    let mut evaluation =
        backtest_anchor_crop(&source_frame.frame, rect, &crop_png, threshold, flags)?;
    evaluation.auto_region = resolution.auto_region;
    fs::create_dir_all(artifact_dir).map_err(|err| {
        CliError::usage(format!(
            "failed to create record artifact dir {}: {err}",
            artifact_dir.display()
        ))
    })?;
    let artifact_path = artifact_dir.join(format!(
        "anchor-{}-{}.png",
        safe_file_stem(step_id),
        safe_file_stem(anchor_id)
    ));
    fs::write(&artifact_path, &crop_png).map_err(|err| {
        CliError::usage(format!(
            "failed to write record anchor artifact {}: {err}",
            artifact_path.display()
        ))
    })?;
    Ok(MaterializedAnchorArtifact {
        region: SessionRecordRegion::Rect {
            rect: resolution.rect.clone(),
        },
        frame_provenance: session_record_frame_provenance(source_frame),
        artifact: SessionRecordAnchorArtifact {
            kind: "template_crop".to_string(),
            path: artifact_path.display().to_string(),
            sha256: hex_sha256(&crop_png),
            width: crop.width,
            height: crop.height,
            region: resolution.rect,
        },
        evaluation,
    })
}

fn backtest_anchor_crop(
    frame: &Frame,
    rect: &SessionRecordRect,
    crop_png: &[u8],
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordStepEvaluation> {
    let metric = parse_match_metric_flag(flags)?;
    let threshold = threshold.unwrap_or(0.95) as f32;
    let backtest = match_anchor_crop_in_frame(
        frame,
        rect,
        crop_png,
        metric,
        threshold,
        "local_png_self_test",
    )?;
    let contrast_backtest =
        backtest_contrast_anchor_crop(rect, crop_png, metric, threshold, flags)?;
    let positive_passed = backtest.passed;
    let contrast_passed = contrast_backtest
        .as_ref()
        .map(|backtest| backtest.passed)
        .unwrap_or(true);
    let passed = positive_passed && contrast_passed;
    let reason = if !positive_passed {
        "self_backtest_below_threshold"
    } else if !contrast_passed {
        "contrast_backtest_matched"
    } else if contrast_backtest.is_some() {
        "self_and_contrast_backtest_passed"
    } else {
        "self_backtest_passed"
    };
    Ok(SessionRecordStepEvaluation {
        status: if passed { "passed" } else { "failed" }.to_string(),
        reason: reason.to_string(),
        auto_region: None,
        backtest: Some(backtest),
        contrast_backtest,
    })
}

fn resolve_session_record_anchor_rect(
    frame: &Frame,
    region: &SessionRecordRegion,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordAnchorRegionResolution> {
    match region {
        SessionRecordRegion::Auto => auto_session_record_anchor_rect(frame, threshold, flags),
        SessionRecordRegion::Rect { rect } => Ok(SessionRecordAnchorRegionResolution {
            rect: rect.clone(),
            auto_region: None,
        }),
    }
}

fn auto_session_record_anchor_rect(
    frame: &Frame,
    threshold: Option<f64>,
    flags: &FlagArgs,
) -> CliOutcome<SessionRecordAnchorRegionResolution> {
    if frame.width == 0 || frame.height == 0 {
        return Err(CliError::usage(
            "record anchor auto region requires a non-empty source frame",
        ));
    }
    let width = auto_session_record_axis_len(frame.width);
    let height = auto_session_record_axis_len(frame.height);
    let contrast_frame = read_session_record_contrast_frame(flags)?;
    let metric = if contrast_frame.is_some() {
        Some(parse_match_metric_flag(flags)?)
    } else {
        None
    };
    let match_threshold = threshold.unwrap_or(0.95) as f32;
    let mut candidates = Vec::new();
    for y in auto_session_record_axis_positions(frame.height, height) {
        for x in auto_session_record_axis_positions(frame.width, width) {
            let rect = SessionRecordRect {
                x: i32::try_from(x)
                    .map_err(|_| CliError::usage("record anchor auto x exceeds i32"))?,
                y: i32::try_from(y)
                    .map_err(|_| CliError::usage("record anchor auto y exceeds i32"))?,
                width: i32::try_from(width)
                    .map_err(|_| CliError::usage("record anchor auto width exceeds i32"))?,
                height: i32::try_from(height)
                    .map_err(|_| CliError::usage("record anchor auto height exceeds i32"))?,
            };
            let score = score_session_record_region_luma_variance(frame, &rect)?;
            let (contrast_score, contrast_passed) = if let Some(contrast_frame) = &contrast_frame {
                let crop = crop_frame_rect(frame, &rect)?;
                let crop_png = crop.png_for_artifact().map_err(|err| {
                    CliError::usage(format!("failed to encode record auto-region crop: {err}"))
                })?;
                let backtest = match_anchor_crop_in_frame(
                    &contrast_frame.frame,
                    &rect,
                    &crop_png,
                    metric.ok_or_else(|| {
                        CliError::usage("record auto-region contrast scoring requires match metric")
                    })?,
                    match_threshold,
                    "auto_region_contrast",
                )?;
                (Some(backtest.score), Some(backtest.score < match_threshold))
            } else {
                (None, None)
            };
            candidates.push(SessionRecordAutoRegionCandidate {
                region: rect,
                luma_variance: score,
                contrast_score,
                contrast_passed,
                selected: false,
            });
        }
    }
    let Some((selected_index, selected_reason)) =
        select_session_record_auto_region_candidate(&candidates, contrast_frame.is_some())
    else {
        return Err(CliError::usage(
            "record anchor auto region produced no candidates",
        ));
    };
    let selected = candidates[selected_index].region.clone();
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.selected = index == selected_index;
    }
    Ok(SessionRecordAnchorRegionResolution {
        rect: selected.clone(),
        auto_region: Some(SessionRecordAutoRegionSelection {
            strategy: "bounded_luma_variance_grid_v1".to_string(),
            selected_reason,
            selected,
            candidates,
        }),
    })
}

fn select_session_record_auto_region_candidate(
    candidates: &[SessionRecordAutoRegionCandidate],
    has_contrast: bool,
) -> Option<(usize, String)> {
    let has_discriminating_candidate = candidates
        .iter()
        .any(|candidate| candidate.contrast_passed == Some(true));
    let selected_reason = if has_discriminating_candidate {
        "contrast_rejected_highest_variance"
    } else if has_contrast {
        "lowest_contrast_score"
    } else {
        "highest_luma_variance"
    };
    let mut selected = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(best_index) = selected else {
            selected = Some(index);
            continue;
        };
        if session_record_auto_region_candidate_is_better(
            candidate,
            &candidates[best_index],
            has_discriminating_candidate,
            has_contrast,
        ) {
            selected = Some(index);
        }
    }
    selected.map(|index| (index, selected_reason.to_string()))
}

fn session_record_auto_region_candidate_is_better(
    candidate: &SessionRecordAutoRegionCandidate,
    best: &SessionRecordAutoRegionCandidate,
    prefer_discriminating: bool,
    prefer_lowest_contrast: bool,
) -> bool {
    if prefer_discriminating {
        let candidate_passed = candidate.contrast_passed == Some(true);
        let best_passed = best.contrast_passed == Some(true);
        if candidate_passed != best_passed {
            return candidate_passed;
        }
    }
    if prefer_lowest_contrast {
        match (candidate.contrast_score, best.contrast_score) {
            (Some(candidate_score), Some(best_score))
                if (candidate_score - best_score).abs() > f32::EPSILON =>
            {
                return candidate_score < best_score;
            }
            (Some(_), None) => return true,
            (None, Some(_)) => return false,
            _ => {}
        }
    }
    if (candidate.luma_variance - best.luma_variance).abs() > f64::EPSILON {
        return candidate.luma_variance > best.luma_variance;
    }
    (candidate.region.y, candidate.region.x) < (best.region.y, best.region.x)
}

fn auto_session_record_axis_len(total: u32) -> u32 {
    (total / 3).max(1).min(total)
}

fn auto_session_record_axis_positions(total: u32, len: u32) -> Vec<u32> {
    if total <= len {
        return vec![0];
    }
    let end = total - len;
    let mut positions = vec![0, end / 2, end];
    positions.sort_unstable();
    positions.dedup();
    positions
}

fn score_session_record_region_luma_variance(
    frame: &Frame,
    rect: &SessionRecordRect,
) -> CliOutcome<f64> {
    let stride = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let x = usize::try_from(rect.x)
        .map_err(|_| CliError::usage("record anchor auto rect x exceeds usize"))?;
    let y = usize::try_from(rect.y)
        .map_err(|_| CliError::usage("record anchor auto rect y exceeds usize"))?;
    let width = usize::try_from(rect.width)
        .map_err(|_| CliError::usage("record anchor auto rect width exceeds usize"))?;
    let height = usize::try_from(rect.height)
        .map_err(|_| CliError::usage("record anchor auto rect height exceeds usize"))?;
    let frame_width = usize::try_from(frame.width)
        .map_err(|_| CliError::usage("record source frame width exceeds usize"))?;
    let mut count = 0f64;
    let mut sum = 0f64;
    let mut sum_sq = 0f64;
    for row in 0..height {
        for col in 0..width {
            let column = x
                .checked_add(col)
                .ok_or_else(|| CliError::usage("record anchor auto score column overflow"))?;
            let offset = ((y + row)
                .checked_mul(frame_width)
                .and_then(|value| value.checked_add(column))
                .and_then(|value| value.checked_mul(stride)))
            .ok_or_else(|| CliError::usage("record anchor auto score offset overflow"))?;
            let r = f64::from(frame.pixels[offset]);
            let g = f64::from(frame.pixels[offset + 1]);
            let b = f64::from(frame.pixels[offset + 2]);
            let luma = (r + g + b) / 3.0;
            count += 1.0;
            sum += luma;
            sum_sq += luma * luma;
        }
    }
    if count == 0.0 {
        return Err(CliError::usage(
            "record anchor auto region cannot score an empty candidate",
        ));
    }
    let mean = sum / count;
    Ok((sum_sq / count) - (mean * mean))
}

fn match_anchor_crop_in_frame(
    frame: &Frame,
    rect: &SessionRecordRect,
    crop_png: &[u8],
    metric: MatchMetric,
    threshold: f32,
    source: &str,
) -> CliOutcome<SessionRecordAnchorBacktest> {
    let scene = scene_from_frame(frame)?;
    let matched = scene
        .match_template_with_metric(
            crop_png,
            Some(RecognitionRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            }),
            metric,
        )
        .map_err(|err| CliError::usage(format!("failed to backtest record anchor crop: {err}")))?;
    Ok(SessionRecordAnchorBacktest {
        source: source.to_string(),
        metric: match_metric_name(metric).to_string(),
        region: rect.clone(),
        x: matched.x,
        y: matched.y,
        raw_score: matched.raw_score,
        score: matched.score,
        threshold,
        passed: matched.score >= threshold,
    })
}

fn backtest_contrast_anchor_crop(
    rect: &SessionRecordRect,
    crop_png: &[u8],
    metric: MatchMetric,
    threshold: f32,
    flags: &FlagArgs,
) -> CliOutcome<Option<SessionRecordAnchorContrastBacktest>> {
    let Some(contrast_frame) = read_session_record_contrast_frame(flags)? else {
        return Ok(None);
    };
    let backtest = match_anchor_crop_in_frame(
        &contrast_frame.frame,
        rect,
        crop_png,
        metric,
        threshold,
        "local_png_contrast",
    )?;
    Ok(Some(SessionRecordAnchorContrastBacktest {
        source: "local_png_contrast".to_string(),
        path: contrast_frame.path.display().to_string(),
        sha256: contrast_frame.sha256,
        width: contrast_frame.frame.width,
        height: contrast_frame.frame.height,
        metric: backtest.metric,
        region: backtest.region,
        x: backtest.x,
        y: backtest.y,
        raw_score: backtest.raw_score,
        score: backtest.score,
        threshold: backtest.threshold,
        passed: backtest.score < threshold,
    }))
}

fn read_session_record_contrast_frame(
    flags: &FlagArgs,
) -> CliOutcome<Option<SessionRecordContrastFrame>> {
    let Some(frame_path) = flags
        .optional_path("--contrast-frame")
        .or_else(|| flags.optional_path("--negative-frame"))
    else {
        return Ok(None);
    };
    let frame_png = fs::read(&frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record contrast frame {}: {err}",
            frame_path.display()
        ))
    })?;
    let frame_hash = hex_sha256(&frame_png);
    let frame = Frame::from_png(frame_png, CaptureBackendName::AdbScreencap)
        .map_err(|err| CliError::usage(format!("failed to decode record contrast frame: {err}")))?;
    Ok(Some(SessionRecordContrastFrame {
        frame,
        path: frame_path,
        sha256: frame_hash,
    }))
}

fn crop_frame_rect(frame: &Frame, rect: &SessionRecordRect) -> CliOutcome<Frame> {
    if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(
            "record anchor crop rect must have non-negative origin and positive size",
        ));
    }
    let x = u32::try_from(rect.x).map_err(|_| CliError::usage("record anchor rect x overflow"))?;
    let y = u32::try_from(rect.y).map_err(|_| CliError::usage("record anchor rect y overflow"))?;
    let width = u32::try_from(rect.width)
        .map_err(|_| CliError::usage("record anchor rect width overflow"))?;
    let height = u32::try_from(rect.height)
        .map_err(|_| CliError::usage("record anchor rect height overflow"))?;
    let right = x
        .checked_add(width)
        .ok_or_else(|| CliError::usage("record anchor crop rect x+width overflow"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| CliError::usage("record anchor crop rect y+height overflow"))?;
    if right > frame.width || bottom > frame.height {
        return Err(CliError::usage(format!(
            "record anchor crop rect {}x{} at {},{} exceeds frame {}x{}",
            width, height, x, y, frame.width, frame.height
        )));
    }
    let stride = match frame.pixel_format {
        PixelFormat::Rgb8 => 3usize,
        PixelFormat::Rgba8 => 4usize,
    };
    let frame_width = usize::try_from(frame.width)
        .map_err(|_| CliError::usage("record source frame width exceeds usize"))?;
    let x = usize::try_from(x).map_err(|_| CliError::usage("record anchor x exceeds usize"))?;
    let y = usize::try_from(y).map_err(|_| CliError::usage("record anchor y exceeds usize"))?;
    let width =
        usize::try_from(width).map_err(|_| CliError::usage("record anchor width exceeds usize"))?;
    let height = usize::try_from(height)
        .map_err(|_| CliError::usage("record anchor height exceeds usize"))?;
    let row_bytes = width
        .checked_mul(stride)
        .ok_or_else(|| CliError::usage("record anchor row byte length overflow"))?;
    let mut pixels = Vec::with_capacity(
        row_bytes
            .checked_mul(height)
            .ok_or_else(|| CliError::usage("record anchor crop byte length overflow"))?,
    );
    for row in 0..height {
        let offset = ((y + row)
            .checked_mul(frame_width)
            .and_then(|value| value.checked_add(x))
            .and_then(|value| value.checked_mul(stride)))
        .ok_or_else(|| CliError::usage("record anchor crop offset overflow"))?;
        let end = offset
            .checked_add(row_bytes)
            .ok_or_else(|| CliError::usage("record anchor crop row end overflow"))?;
        pixels.extend_from_slice(&frame.pixels[offset..end]);
    }
    Frame::from_pixels(
        u32::try_from(width).map_err(|_| CliError::usage("record anchor width exceeds u32"))?,
        u32::try_from(height).map_err(|_| CliError::usage("record anchor height exceeds u32"))?,
        pixels,
        frame.pixel_format,
        frame.backend_name,
    )
    .map_err(|err| CliError::usage(format!("failed to build record anchor crop frame: {err}")))
}

fn parse_session_record_operation_click(flags: &FlagArgs) -> CliOutcome<SessionRecordClick> {
    let gesture_flags = [
        flags.optional("--click").is_some(),
        flags
            .optional("--swipe")
            .or_else(|| flags.optional("--drag"))
            .is_some(),
        flags
            .optional("--long-press")
            .or_else(|| flags.optional("--long-tap"))
            .is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if gesture_flags != 1 {
        return Err(CliError::usage(
            "record operation requires exactly one of --click, --swipe/--drag, or --long-press/--long-tap",
        ));
    }
    if let Some(click) = flags.optional("--click").filter(|value| value != "true") {
        return parse_session_record_click(&click);
    }
    if let Some(swipe) = flags
        .optional("--swipe")
        .or_else(|| flags.optional("--drag"))
        .filter(|value| value != "true")
    {
        let (from, to) = parse_session_record_swipe_rects(&swipe)?;
        return Ok(SessionRecordClick::Swipe {
            from,
            to,
            duration_ms: parse_record_duration_ms(flags, 500)?,
        });
    }
    if let Some(long_press) = flags
        .optional("--long-press")
        .or_else(|| flags.optional("--long-tap"))
        .filter(|value| value != "true")
    {
        let (x, y) = parse_point_pair(&long_press)?;
        return Ok(SessionRecordClick::LongPress {
            x,
            y,
            duration_ms: parse_record_duration_ms(flags, 700)?,
        });
    }
    Err(CliError::usage(
        "record operation action parser reached an impossible state",
    ))
}

fn parse_session_record_click(value: &str) -> CliOutcome<SessionRecordClick> {
    if value.trim().is_empty() {
        return Err(CliError::usage("--click must not be empty"));
    }
    if value.contains(',') {
        let (x, y) = parse_point_pair(value)?;
        return Ok(SessionRecordClick::Coord { x, y });
    }
    Ok(SessionRecordClick::Target {
        target: value.to_string(),
    })
}

fn amend_session_record_from_drift_diagnostics(
    context: &SessionRecordAmendContext,
    record: &mut SessionRecordContext,
    flags: &FlagArgs,
    diagnostics_path: PathBuf,
) -> CliOutcome<Value> {
    reject_direct_drift_amend_flags(flags)?;
    let diagnostics = read_session_record_drift_diagnostics(&diagnostics_path)?;
    let selector = flags
        .optional("--step-id")
        .filter(|value| value != "true")
        .or_else(|| flags.positionals.first().cloned());
    if flags.positionals.len() > 1 {
        return Err(CliError::usage(
            "session record amend --from-drift-diagnostics accepts at most one positional selector",
        ));
    }
    let step_index = find_drift_amend_step(record, &diagnostics, selector.as_deref())?;
    let mut amended_step = record.steps[step_index].clone();
    let resource_kind = amend_drift_record_step(context, &mut amended_step, flags, &diagnostics)?;
    let step_id = amended_step.step_id.clone();
    record.steps[step_index] = amended_step;
    Ok(json!({
        "schema_version": "session.record_drift_amend.v0.1",
        "diagnostics_path": diagnostics.path.display().to_string(),
        "target_id": diagnostics.target_id,
        "step_id": step_id,
        "resource_kind": resource_kind,
        "changed_fields": diagnostics.changed_fields,
        "region": diagnostics.region,
        "threshold": diagnostics.threshold,
        "build_task_command": "session record build-task"
    }))
}

fn reject_direct_drift_amend_flags(flags: &FlagArgs) -> CliOutcome<()> {
    const ALLOWED: &[&str] = &[
        "--from-drift-diagnostics",
        "--state-dir",
        "--step-id",
        "--holder",
        "--lease-holder",
        "--lease-id",
        "--contrast-frame",
    ];
    let unsupported = flags
        .flags
        .keys()
        .filter(|name| !ALLOWED.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(CliError::usage(format!(
            "session record amend --from-drift-diagnostics only accepts drift diagnostics changes; unsupported direct flags: {}",
            unsupported.join(", ")
        )));
    }
    Ok(())
}

fn read_session_record_drift_diagnostics(path: &Path) -> CliOutcome<SessionRecordDriftDiagnostics> {
    let Some(value) = read_json_file::<Value>(path)? else {
        return Err(CliError::usage(format!(
            "session record amend drift diagnostics file is missing: {}",
            path.display()
        )));
    };
    parse_session_record_drift_diagnostics(path.to_path_buf(), &value)
}

pub(crate) fn parse_session_record_drift_diagnostics(
    path: PathBuf,
    value: &Value,
) -> CliOutcome<SessionRecordDriftDiagnostics> {
    let trigger = value
        .get("trigger")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::usage("drift diagnostics must include trigger: resource_drift"))?;
    if trigger != "resource_drift" {
        return Err(CliError::usage(format!(
            "drift diagnostics trigger must be resource_drift, got {trigger}"
        )));
    }
    let target_id = value
        .get("target_id")
        .or_else(|| value.pointer("/guard/target_id"))
        .and_then(Value::as_str)
        .filter(|target_id| !target_id.trim().is_empty())
        .ok_or_else(|| CliError::usage("drift diagnostics must include target_id"))?
        .to_string();
    let proposed = value.get("proposed_changes");
    let (threshold, proposed_region) = parse_drift_proposed_changes(proposed)?;
    let region = proposed_region
        .or_else(|| {
            value
                .pointer("/measured/matched_rect")
                .map(|rect| parse_session_record_rect_value(rect, "measured.matched_rect"))
        })
        .transpose()?
        .ok_or_else(|| {
            CliError::usage(
                "drift diagnostics must include proposed_changes.region or measured.matched_rect",
            )
        })?;
    let mut changed_fields = vec!["region"];
    if threshold.is_some() {
        changed_fields.push("threshold");
    }
    Ok(SessionRecordDriftDiagnostics {
        path,
        target_id,
        region,
        threshold,
        changed_fields,
    })
}

fn parse_drift_proposed_changes(
    proposed: Option<&Value>,
) -> CliOutcome<(Option<f64>, Option<CliOutcome<SessionRecordRect>>)> {
    let Some(proposed) = proposed else {
        return Ok((None, None));
    };
    let object = proposed.as_object().ok_or_else(|| {
        CliError::usage("drift diagnostics proposed_changes must be an object when provided")
    })?;
    let mut unsupported = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "region" | "threshold"))
        .cloned()
        .collect::<Vec<_>>();
    unsupported.sort();
    if !unsupported.is_empty() {
        return Err(CliError::usage(format!(
            "drift diagnostics proposed_changes contains fields outside the amend whitelist: {}",
            unsupported.join(", ")
        )));
    }
    let threshold = object
        .get("threshold")
        .map(|value| parse_unit_f64_value(value, "proposed_changes.threshold"))
        .transpose()?;
    let region = object
        .get("region")
        .map(|value| parse_session_record_region_value(value, "proposed_changes.region"));
    Ok((threshold, region))
}

fn parse_session_record_region_value(value: &Value, label: &str) -> CliOutcome<SessionRecordRect> {
    if value.get("mode").and_then(Value::as_str) == Some("rect") {
        let rect = value
            .get("rect")
            .ok_or_else(|| CliError::usage(format!("{label}.rect is missing")))?;
        return parse_session_record_rect_value(rect, label);
    }
    parse_session_record_rect_value(value, label)
}

fn parse_session_record_rect_value(value: &Value, label: &str) -> CliOutcome<SessionRecordRect> {
    let field = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_i64)
            .ok_or_else(|| CliError::usage(format!("{label}.{name} must be an integer")))
    };
    let to_i32 = |name: &str, raw: i64| {
        i32::try_from(raw).map_err(|_| {
            CliError::usage(format!("{label}.{name} is outside the supported i32 range"))
        })
    };
    let rect = SessionRecordRect {
        x: to_i32("x", field("x")?)?,
        y: to_i32("y", field("y")?)?,
        width: to_i32("width", field("width")?)?,
        height: to_i32("height", field("height")?)?,
    };
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::usage(format!(
            "{label} dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    Ok(rect)
}

fn parse_unit_f64_value(value: &Value, label: &str) -> CliOutcome<f64> {
    let parsed = value
        .as_f64()
        .ok_or_else(|| CliError::usage(format!("{label} must be a number")))?;
    if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
        return Err(CliError::usage(format!(
            "{label} must be a finite number between 0 and 1"
        )));
    }
    Ok(parsed)
}

pub(crate) fn find_drift_amend_step(
    record: &SessionRecordContext,
    diagnostics: &SessionRecordDriftDiagnostics,
    selector: Option<&str>,
) -> CliOutcome<usize> {
    let mut matches = record
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| drift_step_matches_target(step, &diagnostics.target_id))
        .filter(|(_, step)| {
            selector.is_none_or(|selector| {
                step.step_id == selector || drift_step_matches_target(step, selector)
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(CliError::safety_blocked(
            "record_drift_target_not_found",
            format!(
                "no anchor or verify-template record step matches drift target '{}'",
                diagnostics.target_id
            ),
            &["session_record", "resource_drift"],
        ));
    }
    if matches.len() > 1 {
        let step_ids = matches
            .iter()
            .map(|index| record.steps[*index].step_id.as_str())
            .collect::<Vec<_>>();
        return Err(CliError::safety_blocked(
            "record_drift_target_ambiguous",
            format!(
                "drift target '{}' matches multiple record steps: {}",
                diagnostics.target_id,
                step_ids.join(", ")
            ),
            &["session_record", "resource_drift"],
        ));
    }
    Ok(matches.remove(0))
}

fn drift_step_matches_target(step: &SessionRecordStep, target: &str) -> bool {
    match &step.data {
        SessionRecordStepData::Anchor { id, .. }
        | SessionRecordStepData::VerifyTemplate { id, .. } => {
            session_record_resource_id_matches_target(id, target)
        }
        SessionRecordStepData::ColorProbe { .. } | SessionRecordStepData::Operation { .. } => false,
    }
}

fn session_record_resource_id_matches_target(id: &str, target: &str) -> bool {
    id == target
        || session_record_anchor_target_id(id) == target
        || target
            .strip_prefix("page/")
            .is_some_and(|stripped| stripped == id)
}

fn amend_drift_record_step(
    context: &SessionRecordAmendContext,
    step: &mut SessionRecordStep,
    flags: &FlagArgs,
    diagnostics: &SessionRecordDriftDiagnostics,
) -> CliOutcome<&'static str> {
    match &mut step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            *region = SessionRecordRegion::Rect {
                rect: diagnostics.region.clone(),
            };
            if let Some(next_threshold) = diagnostics.threshold {
                *threshold = Some(next_threshold);
            }
            let mut target = SessionRecordAnchorAmendTarget {
                id,
                region,
                color_check,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            refresh_amended_anchor_artifact(context, &step.step_id, &mut target, flags, None)?;
            step.updated_at_unix_ms = current_unix_ms();
            Ok("anchor")
        }
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            *region = SessionRecordRegion::Rect {
                rect: diagnostics.region.clone(),
            };
            if let Some(next_threshold) = diagnostics.threshold {
                *threshold = Some(next_threshold);
            }
            let mut target = SessionRecordVerifyTemplateAmendTarget {
                id,
                region,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            refresh_amended_verify_template(context, &step.step_id, &mut target, flags, None)?;
            step.updated_at_unix_ms = current_unix_ms();
            Ok("verify_template")
        }
        SessionRecordStepData::ColorProbe { .. } | SessionRecordStepData::Operation { .. } => {
            Err(CliError::usage(
                "drift diagnostics amend supports only anchor and verify-template record steps",
            ))
        }
    }
}

fn session_record_candidate_report(
    record: &SessionRecordContext,
    step: &SessionRecordStep,
    record_path: &Path,
) -> CliOutcome<Value> {
    let (resource_kind, resource_id, region, evaluation) = match &step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            evaluation,
            ..
        } => ("anchor", id, region, evaluation),
        SessionRecordStepData::ColorProbe {
            id,
            region,
            evaluation,
            ..
        } => ("color_probe", id, region, evaluation),
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            evaluation,
            ..
        } => ("verify_template", id, region, evaluation),
        SessionRecordStepData::Operation { .. } => {
            return Err(CliError::usage(
                "session record candidates requires a resource step with an auto-region candidate report",
            ));
        }
    };
    let Some(auto_region) = &evaluation.auto_region else {
        return Err(CliError::usage(
            "session record candidates requires an existing auto-region candidate report",
        ));
    };
    let selected_index = auto_region
        .candidates
        .iter()
        .position(|candidate| candidate.selected);
    Ok(json!({
        "status": "candidates_listed",
        "record_id": record.record_id.as_str(),
        "task_id": record.task_id.as_str(),
        "instance": record.instance.as_str(),
        "record_status": record.status.as_str(),
        "step_id": step.step_id.as_str(),
        "resource_kind": resource_kind,
        "resource_id": resource_id,
        "anchor_id": resource_id,
        "region": region,
        "evaluation_status": evaluation.status.as_str(),
        "auto_region": auto_region,
        "candidate_count": auto_region.candidates.len(),
        "selected_index": selected_index,
        "path": record_path.display().to_string()
    }))
}

fn amend_session_record_step(
    context: &SessionRecordAmendContext,
    step: &mut SessionRecordStep,
    flags: &FlagArgs,
) -> CliOutcome<()> {
    let step_id = step.step_id.clone();
    let changed = match &mut step.data {
        SessionRecordStepData::Anchor {
            id,
            region,
            color_check,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            let mut target = SessionRecordAnchorAmendTarget {
                id,
                region,
                color_check,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            amend_anchor_record_step(context, &step_id, &mut target, flags)?
        }
        SessionRecordStepData::ColorProbe {
            id,
            region,
            expected,
            frame_provenance,
            evaluation,
        } => {
            let mut target = SessionRecordColorProbeAmendTarget {
                id,
                region,
                expected,
                frame_provenance,
                evaluation,
            };
            amend_color_probe_record_step(&step_id, &mut target, flags)?
        }
        SessionRecordStepData::VerifyTemplate {
            id,
            region,
            threshold,
            frame_provenance,
            artifact,
            evaluation,
        } => {
            let mut target = SessionRecordVerifyTemplateAmendTarget {
                id,
                region,
                threshold,
                frame_provenance,
                artifact,
                evaluation,
            };
            amend_verify_template_record_step(context, &step_id, &mut target, flags)?
        }
        SessionRecordStepData::Operation {
            from,
            to,
            click,
            destructive,
        } => amend_operation_record_step(from, to, click, destructive, flags)?,
    };
    if !changed {
        return Err(CliError::usage(
            "session record amend did not include any supported fields for this step kind",
        ));
    }
    step.updated_at_unix_ms = current_unix_ms();
    Ok(())
}

fn amend_anchor_record_step(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordAnchorAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if flags.bool("--color-check") {
        *target.color_check = true;
        changed = true;
    }
    if flags.bool("--no-color-check") {
        *target.color_check = false;
        changed = true;
    }
    if flags.flags.contains_key("--threshold") {
        *target.threshold = parse_optional_unit_f64(flags, "--threshold")?;
        changed = true;
    }
    if flags.bool("--clear-threshold") {
        *target.threshold = None;
        changed = true;
    }
    if changed {
        refresh_amended_anchor_artifact(context, step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn amend_color_probe_record_step(
    step_id: &str,
    target: &mut SessionRecordColorProbeAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if changed {
        refresh_amended_color_probe(step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn amend_verify_template_record_step(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordVerifyTemplateAmendTarget<'_>,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    let mut auto_region_override = None;
    if let Some(value) = flags.optional("--id").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--id must not be empty"));
        }
        *target.id = value;
        changed = true;
    }
    if let Some(candidate_index) = parse_session_record_candidate_index(flags)? {
        let selection = select_recorded_auto_region_candidate(target.evaluation, candidate_index)?;
        *target.region = SessionRecordRegion::Rect {
            rect: selection.selected.clone(),
        };
        auto_region_override = Some(selection);
        changed = true;
    }
    if let Some(value) = flags.optional("--region").filter(|value| value != "true") {
        *target.region = parse_session_record_region(&value)?;
        auto_region_override = None;
        changed = true;
    }
    if flags.flags.contains_key("--threshold") {
        *target.threshold = parse_optional_unit_f64(flags, "--threshold")?;
        changed = true;
    }
    if flags.bool("--clear-threshold") {
        *target.threshold = None;
        changed = true;
    }
    if changed {
        refresh_amended_verify_template(context, step_id, target, flags, auto_region_override)?;
    }
    Ok(changed)
}

fn select_recorded_auto_region_candidate(
    evaluation: &SessionRecordStepEvaluation,
    candidate_index: usize,
) -> CliOutcome<SessionRecordAutoRegionSelection> {
    let Some(auto_region) = &evaluation.auto_region else {
        return Err(CliError::usage(
            "record amend --candidate-index requires an existing auto-region candidate report",
        ));
    };
    let Some(candidate) = auto_region.candidates.get(candidate_index) else {
        return Err(CliError::usage(format!(
            "record amend candidate index {candidate_index} is out of range for {} candidates",
            auto_region.candidates.len()
        )));
    };
    let mut selection = auto_region.clone();
    selection.selected = candidate.region.clone();
    selection.selected_reason = "operator_selected_candidate".to_string();
    for (index, candidate) in selection.candidates.iter_mut().enumerate() {
        candidate.selected = index == candidate_index;
    }
    Ok(selection)
}

fn refresh_amended_anchor_artifact(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordAnchorAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(
            &source_frame.frame,
            target.region,
            *target.threshold,
            flags,
        )?
    };
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref())?;
    let materialized = materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        target.id,
        *target.threshold,
        flags,
    )?;
    *target.region = materialized.region.clone();
    *target.frame_provenance = Some(Box::new(materialized.frame_provenance));
    *target.artifact = Some(Box::new(materialized.artifact));
    *target.evaluation = materialized.evaluation;
    Ok(())
}

fn refresh_amended_color_probe(
    step_id: &str,
    target: &mut SessionRecordColorProbeAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.expected = None;
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(&source_frame.frame, target.region, None, flags)?
    };
    let expected = mean_session_record_rect_rgb(&source_frame.frame, &resolution.rect)?;
    *target.region = SessionRecordRegion::Rect {
        rect: resolution.rect.clone(),
    };
    *target.expected = Some(expected);
    *target.frame_provenance = Some(Box::new(session_record_frame_provenance(source_frame)));
    *target.evaluation = SessionRecordStepEvaluation {
        status: "passed".to_string(),
        reason: "color_probe_sampled".to_string(),
        auto_region: resolution.auto_region,
        backtest: None,
        contrast_backtest: None,
    };
    if target.id.trim().is_empty() {
        return Err(CliError::usage(format!(
            "record amend color-probe '{step_id}' id must not be empty"
        )));
    }
    Ok(())
}

fn refresh_amended_verify_template(
    context: &SessionRecordAmendContext,
    step_id: &str,
    target: &mut SessionRecordVerifyTemplateAmendTarget<'_>,
    flags: &FlagArgs,
    auto_region_override: Option<SessionRecordAutoRegionSelection>,
) -> CliOutcome<()> {
    let Some(provenance) = target.frame_provenance.as_deref() else {
        *target.artifact = None;
        *target.evaluation = SessionRecordStepEvaluation {
            status: "deferred".to_string(),
            reason: "amended_without_frame_provenance".to_string(),
            auto_region: None,
            backtest: None,
            contrast_backtest: None,
        };
        return Ok(());
    };
    let source_frame = read_session_record_source_frame_from_provenance(provenance)?;
    let resolution = if let Some(auto_region) = auto_region_override {
        SessionRecordAnchorRegionResolution {
            rect: auto_region.selected.clone(),
            auto_region: Some(auto_region),
        }
    } else {
        resolve_session_record_anchor_rect(
            &source_frame.frame,
            target.region,
            *target.threshold,
            flags,
        )?
    };
    let artifact_dir = amended_anchor_artifact_dir(context, target.artifact.as_deref())?;
    let materialized = materialize_anchor_artifact_from_source(
        source_frame,
        resolution,
        &artifact_dir,
        step_id,
        target.id,
        *target.threshold,
        flags,
    )?;
    *target.region = materialized.region.clone();
    *target.frame_provenance = Some(Box::new(materialized.frame_provenance));
    *target.artifact = Some(Box::new(materialized.artifact));
    *target.evaluation = materialized.evaluation;
    Ok(())
}

fn read_session_record_source_frame_from_provenance(
    provenance: &SessionRecordFrameProvenance,
) -> CliOutcome<SessionRecordSourceFrame> {
    let frame_path = PathBuf::from(&provenance.path);
    let frame_png = fs::read(&frame_path).map_err(|err| {
        CliError::usage(format!(
            "failed to read record source frame {} for amend: {err}",
            frame_path.display()
        ))
    })?;
    let backend_name = match provenance.capture_backend.as_deref() {
        Some("nemu_ipc") => CaptureBackendName::NemuIpc,
        Some("droidcast_raw") => CaptureBackendName::DroidcastRaw,
        _ => CaptureBackendName::AdbScreencap,
    };
    let frame = Frame::from_png(frame_png.clone(), backend_name).map_err(|err| {
        CliError::usage(format!(
            "failed to decode record source frame {} for amend: {err}",
            frame_path.display()
        ))
    })?;
    Ok(SessionRecordSourceFrame {
        frame,
        png: frame_png,
        source: provenance.source.clone(),
        path: frame_path,
        recorded_at_unix_ms: provenance.recorded_at_unix_ms,
        capture_backend: provenance.capture_backend.clone(),
        freshness: provenance.freshness.clone(),
        capture_attempts: provenance.capture_attempts.clone(),
    })
}

fn amended_anchor_artifact_dir(
    context: &SessionRecordAmendContext,
    artifact: Option<&SessionRecordAnchorArtifact>,
) -> CliOutcome<PathBuf> {
    let default_dir = session_record_artifact_root(&context.state_dir, &context.record_id);
    let candidate = artifact
        .and_then(|artifact| Path::new(&artifact.path).parent().map(Path::to_path_buf))
        .unwrap_or(default_dir);
    ensure_path_within(
        &context.state_dir,
        &candidate,
        "record amend artifact directory",
        &["record", "artifact_dir"],
    )
}

fn amend_operation_record_step(
    from: &mut String,
    to: &mut Option<String>,
    click: &mut SessionRecordClick,
    destructive: &mut bool,
    flags: &FlagArgs,
) -> CliOutcome<bool> {
    let mut changed = false;
    if let Some(value) = flags.optional("--from").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--from must not be empty"));
        }
        *from = value;
        changed = true;
    }
    if let Some(value) = flags.optional("--to").filter(|value| value != "true") {
        if value.trim().is_empty() {
            return Err(CliError::usage("--to must not be empty"));
        }
        *to = if value == "null" { None } else { Some(value) };
        changed = true;
    }
    if let Some(value) = flags.optional("--click").filter(|value| value != "true") {
        *click = parse_session_record_click(&value)?;
        changed = true;
    }
    if flags.bool("--destructive") {
        *destructive = true;
        changed = true;
    }
    if flags.bool("--non-destructive") {
        *destructive = false;
        changed = true;
    }
    Ok(changed)
}

fn session_record_path(state_dir: &Path, instance_id: &str) -> PathBuf {
    state_dir.join(format!("record-{}.json", safe_file_stem(instance_id)))
}

fn new_session_record(instance: &str, task_id: &str, flags: &FlagArgs) -> SessionRecordContext {
    let now = current_unix_ms();
    let holder = flags
        .optional("--holder")
        .or_else(|| flags.optional("--lease-holder"))
        .filter(|value| value != "true");
    let record_id = flags
        .optional("--record-id")
        .filter(|value| value != "true")
        .unwrap_or_else(|| {
            format!(
                "{now}-{}-{}",
                std::process::id(),
                safe_file_stem(task_id.trim())
            )
        });
    SessionRecordContext {
        schema_version: "session-record-v0".to_string(),
        record_id,
        task_id: task_id.trim().to_string(),
        instance: instance.to_string(),
        status: "active".to_string(),
        holder,
        lease_id: flags.optional("--lease-id").filter(|value| value != "true"),
        started_at_unix_ms: now,
        updated_at_unix_ms: now,
        steps: Vec::new(),
    }
}
