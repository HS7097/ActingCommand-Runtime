// SPDX-License-Identifier: AGPL-3.0-only

//! Observer tools (#338 §四 工具表): ac_overview and ac_material. They only read; like the
//! CLI's reads they record no client.action. Every fact comes from the Runtime and
//! runtime-client unchanged; this module only selects, pages and bounds it. ac_events lives in
//! `events` (Workflow #389 MCP-1). Errors and warnings are outcomes (Workflow #378 A5).

use super::runs;
use super::runtime::{Connected, Location, read_bounded};
use super::tools::{
    self, Arguments, McpBlocker, ToolContext, ToolError, ToolOutcome, ToolSuccess,
    invalid_argument, out_of_range,
};
use crate::codes::{ActingctlCode, ArgumentProblem, MaterialLimit, MaterialState};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CauseRelation, Outcome, RawSource};
use actingcommand_contract::{
    ArtifactId, ArtifactMediaType, EventId, EventQuery, LedgerEventPosition,
    MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectedArtifactReference, ProjectionProfile,
    RuntimeEventQueryPageRequest, RuntimeInstanceStatus, RuntimeMaterialReadFailure,
    RuntimeMaterialReadLimit, RuntimeMaterialReadRequest, RuntimeMaterialReadState,
    RuntimeMonitorInstanceStatus,
};
use actingcommand_runtime_client::{RuntimeMaterialCompleteResult, RuntimeMaterialSelection};
use serde_json::{Map, Value, json};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// `<root>\runtime\BUILD-MANIFEST.json` is read whole up to this size.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 8192;
const DEFAULT_TEXT_BYTES: u64 = 4096;
/// The largest material ac_material exports; ac_observe and `actingctl observe --export` share it.
const MAX_EXPORT_BYTES: usize = crate::material_export::MAX_EXPORT_BYTES;
/// Time kept back from the call budget for writing the exported file.
const EXPORT_WRITE_RESERVE: Duration = Duration::from_secs(2);

pub(super) fn overview(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.string("instance")?;
    let location = context.runtime.locate();
    let mut warnings = Vec::new();
    let install = install_view(&location, &mut warnings);
    let lab_present = location
        .install_root
        .as_ref()
        .is_some_and(|root| root.join("tools").join("actinglab.exe").is_file());
    let mut result = Map::new();
    let connected = match context.runtime.connect() {
        Ok(connected) => connected,
        Err(error) => {
            // No instance data at all, rather than an empty list that reads as an empty
            // system: the daemon section says why, as the tool error `{class, outcome}`.
            result.insert(
                "daemon".to_owned(),
                json!({"online": false, "error": error.into_value()}),
            );
            result.insert("lab_tool".to_owned(), json!({"present": lab_present}));
            result.insert("install".to_owned(), install);
            return Ok(ToolSuccess {
                result: Value::Object(result),
                warnings,
            });
        }
    };
    let status = connected
        .client
        .status()
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let selected = match selector {
        Some(selector) => Some(select_instance(status.instances(), selector)?.instance_alias()),
        None => None,
    };
    let mut incomplete = false;
    let monitors = match connected.client.monitor_status() {
        Ok(monitors) => Some(monitors),
        Err(error) => {
            warnings.push(context.runtime.failure(&connected, &error).into_warning());
            incomplete = true;
            None
        }
    };
    let window = runs::default_window()?;
    let mut instances = Vec::new();
    for instance in status
        .instances()
        .iter()
        .filter(|instance| selected.is_none_or(|alias| instance.instance_alias() == alias))
    {
        let monitor = monitors.as_ref().and_then(|monitors| {
            monitors
                .instances()
                .iter()
                .find(|monitor| monitor.instance_alias() == instance.instance_alias())
        });
        let mut row = instance_row(instance, monitor);
        // At most one brief run status per instance: its newest admitted run in the window,
        // which is the open one when a run is open.
        match runs::latest_run(context, &connected, instance, window) {
            Ok(latest) => {
                if latest.incomplete {
                    incomplete = true;
                    row["run_incomplete"] = json!(true);
                }
                if let Some(run) = latest.runs.first() {
                    row["run"] = json!(run);
                }
            }
            Err(error) => {
                incomplete = true;
                row["run_incomplete"] = json!(true);
                warnings.push(error.into_warning());
            }
        }
        instances.push(row);
    }
    result.insert(
        "daemon".to_owned(),
        json!({"online": true, "owner_epoch": status.owner_epoch()}),
    );
    if let Some(pause) = status.scheduling_pause() {
        result.insert("global_pause".to_owned(), json!(pause));
    }
    result.insert("instances".to_owned(), Value::Array(instances));
    result.insert("run_window".to_owned(), json!({"since_unix_ms": window}));
    result.insert("lab_tool".to_owned(), json!({"present": lab_present}));
    result.insert("install".to_owned(), install);
    if incomplete {
        result.insert("incomplete".to_owned(), json!(true));
    }
    Ok(ToolSuccess {
        result: Value::Object(result),
        warnings,
    })
}

fn instance_row(
    instance: &RuntimeInstanceStatus,
    monitor: Option<&RuntimeMonitorInstanceStatus>,
) -> Value {
    let mut row = Map::new();
    row.insert("alias".to_owned(), json!(instance.instance_alias()));
    row.insert("instance_id".to_owned(), json!(instance.instance_id()));
    if let Some(port) = instance.adb_port() {
        row.insert("adb_port".to_owned(), json!(port));
    }
    if let Some(game_id) = instance.game_id() {
        row.insert("game_id".to_owned(), json!(game_id));
    }
    row.insert("lease_active".to_owned(), json!(instance.lease_active()));
    row.insert("queued".to_owned(), json!(instance.queued_request_count()));
    if let Some(pause) = instance.pause() {
        // Data members whose names a code would take (`stage`, `*_reason`) are inserted by
        // name, outside `json!`.
        let mut members = Map::new();
        members.insert("revision".to_owned(), json!(pause.revision));
        members.insert("stage".to_owned(), json!(pause.stage));
        members.insert("pause_reason".to_owned(), json!(pause.pause_reason));
        row.insert("pause".to_owned(), Value::Object(members));
    }
    // Workflow #403: an instance an unconfirmed close fenced, and whether it waits for a person.
    if let Some(fence) = instance.resource_fence() {
        row.insert(
            "resource_fence".to_owned(),
            json!({
                "cause": fence.cause,
                "since_unix_ms": fence.since_unix_ms,
                "awaiting_person": fence.awaiting_person,
            }),
        );
    }
    if let Some(monitor) = monitor.filter(|monitor| monitor.policy().is_some()) {
        row.insert(
            "monitor".to_owned(),
            json!({"policy": monitor.policy(), "state": monitor.state()}),
        );
    }
    Value::Object(row)
}

/// The instance an `instance` argument names by alias, instance_id or ADB port, as the
/// Runtime's status lists them: the selector the CLI shares (`crate::instance_selector`).
pub(super) fn select_instance<'s>(
    instances: &'s [RuntimeInstanceStatus],
    selector: &str,
) -> Result<&'s RuntimeInstanceStatus, ToolError> {
    crate::instance_selector::select(instances, selector)
        .map_err(|miss| ToolError::outcome("usage", miss.outcome(selector)))
}

fn install_view(location: &Location, warnings: &mut Vec<Value>) -> Value {
    let mut install = Map::new();
    if let Some(root) = &location.root {
        install.insert("root".to_owned(), json!(root.display().to_string()));
    }
    if let Ok(state_root) = &location.state_root {
        install.insert(
            "state_root".to_owned(),
            json!(state_root.display().to_string()),
        );
    }
    let manifest = location
        .root
        .as_ref()
        .map(|root| root.join("runtime").join("BUILD-MANIFEST.json"))
        .filter(|manifest| manifest.is_file());
    if let Some(manifest) = manifest {
        // An unreadable manifest is the warning `install_build_manifest_unreadable` (`path`),
        // caused by the read failure or the parser's `foreign_data_error`.
        let document = read_bounded(&manifest, MAX_MANIFEST_BYTES).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string())
                    .path(&manifest)
            })
        });
        match document {
            Ok(document) => {
                let mut build = Map::new();
                for key in ["commit_sha", "source_artifact_name"] {
                    if let Some(value) = document.get(key).filter(|value| value.is_string()) {
                        build.insert(key.to_owned(), value.clone());
                    }
                }
                install.insert("build".to_owned(), Value::Object(build));
            }
            Err(cause) => warnings.push(tools::warning(
                &Outcome::new(ActingctlCode::InstallBuildManifestUnreadable)
                    .path(&manifest)
                    .caused_by(cause),
            )),
        }
    }
    Value::Object(install)
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// One committed material, found again from its ref at the ledger's current snapshot (or, for
/// ac_observe, the frame its observation just recorded).
pub(super) struct Material {
    pub(super) position: LedgerEventPosition,
    pub(super) artifact: ProjectedArtifactReference,
    pub(super) snapshot: u64,
}

pub(super) fn material(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["ref", "mode", "offset", "max_bytes"])?;
    let reference = arguments.required_string("ref")?;
    let mode = arguments.required_string("mode")?;
    let (position, artifact_id) = parse_material_ref(reference)?;
    match mode {
        "text" => {
            let offset = arguments.integer("offset", 0, u64::MAX)?.unwrap_or(0);
            let max_bytes = arguments
                .integer("max_bytes", 1, MAX_TEXT_BYTES)?
                .unwrap_or(DEFAULT_TEXT_BYTES);
            let connected = context.runtime.connect()?;
            let material = resolve_material(context, &connected, position, artifact_id)?;
            material_text(context, &connected, &material, offset, max_bytes)
        }
        "export" => {
            // offset and max_bytes belong to mode text; export writes the whole material.
            for argument in ["offset", "max_bytes"] {
                if arguments.has(argument) {
                    return Err(invalid_argument(argument, ArgumentProblem::Conflicting));
                }
            }
            let connected = context.runtime.connect()?;
            let material = resolve_material(context, &connected, position, artifact_id)?;
            material_export(context, &connected, &material)
        }
        _ => Err(invalid_argument("mode", ArgumentProblem::NotRecognised)),
    }
}

/// `<seq>:<event_id>:<artifact_id>`, a materials[].ref of ac_events; anything else is
/// `material_ref_invalid`.
fn parse_material_ref(reference: &str) -> Result<(LedgerEventPosition, ArtifactId), ToolError> {
    let invalid = || ToolError::usage(ActingctlCode::MaterialRefInvalid);
    let mut parts = reference.split(':');
    let (Some(sequence), Some(event_id), Some(artifact_id), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    let sequence = sequence.parse::<u64>().map_err(|_| invalid())?;
    let event_id = serde_json::from_value::<EventId>(json!(event_id)).map_err(|_| invalid())?;
    let artifact_id =
        serde_json::from_value::<ArtifactId>(json!(artifact_id)).map_err(|_| invalid())?;
    Ok((LedgerEventPosition { event_id, sequence }, artifact_id))
}

fn resolve_material(
    context: &ToolContext<'_>,
    connected: &Connected,
    position: LedgerEventPosition,
    artifact_id: ArtifactId,
) -> Result<Material, ToolError> {
    let query = EventQuery {
        from_sequence: Some(position.sequence),
        to_sequence: Some(position.sequence),
        ..EventQuery::default()
    };
    let request = RuntimeEventQueryPageRequest::new(1, None).map_err(|error| {
        ToolError::new("runtime", ActingctlCode::MaterialQueryInvalid)
            .caused_by(Outcome::new(error.code()))
    })?;
    let page = connected
        .client
        .query_event_page(query, ProjectionProfile::Concise, request)
        .map_err(|error| context.runtime.failure(connected, &error))?;
    let artifact = page
        .events()
        .first()
        .filter(|event| event.event_id == position.event_id)
        .and_then(|event| {
            event
                .artifacts
                .iter()
                .find(|artifact| artifact.artifact_id == artifact_id)
        })
        .cloned()
        .ok_or_else(|| {
            // The ledger holds no such event and material at this sequence.
            ToolError::usage(ActingctlCode::MaterialRefUnknown).with(|outcome| {
                outcome
                    .event_id(text_of(json!(position.event_id)))
                    .artifact_id(text_of(json!(artifact_id)))
            })
        })?;
    Ok(Material {
        position,
        artifact,
        snapshot: page.snapshot_ledger_position(),
    })
}

fn material_text(
    context: &ToolContext<'_>,
    connected: &Connected,
    material: &Material,
    offset: u64,
    max_bytes: u64,
) -> ToolOutcome {
    let artifact = &material.artifact;
    if !matches!(
        artifact.media_type,
        ArtifactMediaType::ApplicationJson | ArtifactMediaType::TextPlain
    ) {
        // Read it with mode export; its media type travels as `media_type`.
        return Err(ToolError::usage(ActingctlCode::MaterialNotText)
            .with(|outcome| outcome.media_type(text_of(json!(artifact.media_type))))
            .blocked_by(McpBlocker::MaterialMode));
    }
    if offset >= artifact.byte_count {
        // At or past the end: the offsets this material has.
        return Err(out_of_range(
            "offset",
            0,
            artifact.byte_count.saturating_sub(1),
        ));
    }
    let length = max_bytes.min(artifact.byte_count - offset);
    let request = RuntimeMaterialReadRequest {
        event: material.position,
        artifact_id: artifact.artifact_id,
        snapshot_position: material.snapshot,
        byte_count: artifact.byte_count,
        sha256: artifact.sha256.clone(),
        expected_run_id: None,
        expected_frame_id: None,
        expected_request_id: None,
        expected_correlation_id: None,
        offset,
        requested_length: u32::try_from(length)
            .map_err(|_| out_of_range("max_bytes", 1, MAX_TEXT_BYTES))?,
        max_reply_bytes: MAX_RUNTIME_MATERIAL_REPLY_BYTES,
    };
    let read = connected
        .client
        .read_material(request)
        .map_err(|error| context.runtime.failure(connected, &error))?;
    let bytes = match (read.state, read.chunk) {
        (RuntimeMaterialReadState::Verified, Some(chunk)) => chunk.bytes,
        (state, _) => {
            return Err(material_unavailable(
                state,
                read.limit,
                read.failure.as_ref(),
            ));
        }
    };
    // The bytes at this offset are not UTF-8 text: start at 0 or at a next_offset, or use
    // mode export.
    let text = utf8_prefix(&bytes).ok_or_else(|| {
        ToolError::usage(ActingctlCode::MaterialNotText).with(|outcome| outcome.offset(offset))
    })?;
    let mut end = text.len();
    loop {
        let returned = &text[..end];
        let mut result = json!({
            "text": returned,
            "offset": offset,
            "bytes": returned.len(),
            "total_bytes": artifact.byte_count,
            "media_type": artifact.media_type,
        });
        let next_offset = offset + returned.len() as u64;
        if next_offset < artifact.byte_count {
            result["next_offset"] = json!(next_offset);
        }
        if end == 0 || tools::fits_success(&result) {
            return Ok(ToolSuccess::new(result));
        }
        // Escaped text can outgrow the budget; return less, ending on a character boundary.
        end = floor_char_boundary(text, end - (end / 8).max(1));
    }
}

/// The longest UTF-8 prefix of a range that may end inside a character.
fn utf8_prefix(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if error.error_len().is_none() && error.valid_up_to() > 0 => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) fn material_export(
    context: &ToolContext<'_>,
    connected: &Connected,
    material: &Material,
) -> ToolOutcome {
    let artifact = &material.artifact;
    if artifact.byte_count > MAX_EXPORT_BYTES as u64 {
        return Err(
            ToolError::usage(ActingctlCode::MaterialTooLarge).with(|outcome| {
                outcome
                    .actual_bytes(artifact.byte_count)
                    .limit_bytes(MAX_EXPORT_BYTES)
            }),
        );
    }
    let selection = RuntimeMaterialSelection {
        event: material.position,
        artifact_id: artifact.artifact_id,
        snapshot_position: material.snapshot,
        sha256: artifact.sha256.clone(),
        byte_count: artifact.byte_count,
        run_id: None,
        frame_id: None,
        request_id: None,
        correlation_id: None,
    };
    let deadline = context
        .deadline
        .checked_sub(EXPORT_WRITE_RESERVE)
        .unwrap_or(context.deadline);
    let keep_reading = || !context.cancelled.load(Ordering::SeqCst);
    match connected.client.read_material_complete(
        selection,
        MAX_EXPORT_BYTES,
        deadline,
        &keep_reading,
    ) {
        RuntimeMaterialCompleteResult::Verified { bytes, .. } => write_export(artifact, &bytes),
        // A client failure during the read keeps the client's outcome on top; what the read
        // reached is its `note` link `material_unavailable`.
        RuntimeMaterialCompleteResult::NotProvided {
            limit,
            failure,
            error,
            ..
        } => Err(match error {
            Some(error) => context.runtime.failure(connected, &error).with(|outcome| {
                outcome.link(
                    CauseRelation::Note,
                    unavailable_outcome(
                        RuntimeMaterialReadState::NotProvided,
                        Some(limit),
                        failure.as_ref(),
                    ),
                )
            }),
            None => material_unavailable(
                RuntimeMaterialReadState::NotProvided,
                Some(limit),
                failure.as_ref(),
            ),
        }),
        RuntimeMaterialCompleteResult::Failed {
            state,
            failure,
            error,
            ..
        } => Err(context.runtime.failure(connected, &error).with(|outcome| {
            outcome.link(
                CauseRelation::Note,
                unavailable_outcome(state, None, failure.as_ref()),
            )
        })),
    }
}

/// The Runtime answered the read without the material: `material_unavailable`, class runtime
/// (see [`unavailable_outcome`]).
fn material_unavailable(
    state: RuntimeMaterialReadState,
    limit: Option<RuntimeMaterialReadLimit>,
    failure: Option<&RuntimeMaterialReadFailure>,
) -> ToolError {
    ToolError::outcome("runtime", unavailable_outcome(state, limit, failure))
}

/// `material_unavailable` (Workflow #378 A5): the read's state (`material_state`) and, when
/// the read stopped short, its limit (`material_limit`), caused by the Runtime's typed failure
/// code when it gave one (relayed as the Runtime spelled it).
fn unavailable_outcome(
    state: RuntimeMaterialReadState,
    limit: Option<RuntimeMaterialReadLimit>,
    failure: Option<&RuntimeMaterialReadFailure>,
) -> Outcome {
    let outcome =
        Outcome::new(ActingctlCode::MaterialUnavailable).material_state(MaterialState::of(state));
    let outcome = match limit {
        Some(limit) => outcome.material_limit(MaterialLimit::of(limit)),
        None => outcome,
    };
    match failure {
        Some(failure) => outcome.caused_by(Outcome::relayed(&failure.code)),
        None => outcome,
    }
}

/// Writes the verified bytes to `%TEMP%\actingcommand-mcp\materials\<sha256>.<ext>`
/// through a partial file moved into place.
fn write_export(artifact: &ProjectedArtifactReference, bytes: &[u8]) -> ToolOutcome {
    export_bytes(&artifact.sha256, artifact.kind.extension(), bytes).map(ToolSuccess::new)
}

/// Where every export of this server goes (ac_material export, Lab answers over budget).
pub(super) fn export_directory() -> PathBuf {
    env::temp_dir().join("actingcommand-mcp").join("materials")
}

/// The export writer (`crate::material_export::write`, which `actingctl observe --export`
/// shares) on the export directory: `bytes`, whose `sha256` (`sha256:<hex>`) the caller knows, to
/// `<export directory>\<hex>.<extension>` through a partial file moved into place;
/// `{path, sha256, size}`. A failed write is the writer's `material_export_failed`, class
/// runtime.
pub(super) fn export_bytes(
    sha256: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Value, ToolError> {
    crate::material_export::write(&export_directory(), sha256, extension, bytes)
        .map_err(|failure| ToolError::outcome("runtime", failure.outcome()))
}
