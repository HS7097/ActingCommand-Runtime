// SPDX-License-Identifier: AGPL-3.0-only

//! Observer tools (#338 §四 工具表): ac_overview and ac_material. They only read; like the
//! CLI's reads they record no client.action. Every fact comes from the Runtime and
//! runtime-client unchanged; this module only selects, pages and bounds it. ac_events lives in
//! `events` (Workflow #389 MCP-1).

use super::runs;
use super::runtime::{Connected, Location, read_bounded};
use super::tools::{
    self, Arguments, ToolCode, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::ActingctlCode;
use crate::events::CursorBinding;
use actingcommand_contract::{
    ArtifactId, ArtifactMediaType, EventId, EventQuery, LedgerEventPosition,
    MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectedArtifactReference, ProjectionProfile,
    RuntimeEventQueryCursor, RuntimeEventQueryPageRequest, RuntimeInstanceStatus,
    RuntimeMaterialReadFailure, RuntimeMaterialReadLimit, RuntimeMaterialReadRequest,
    RuntimeMaterialReadState, RuntimeMonitorInstanceStatus,
};
use actingcommand_runtime_client::{RuntimeMaterialCompleteResult, RuntimeMaterialSelection};
use serde_json::{Map, Value, json};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

// An ac_events row, as `actingctl events` prints it (Workflow #389 MCP-1).
pub(super) use crate::events::brief_row as event_row;

/// `<root>\runtime\BUILD-MANIFEST.json` is read whole up to this size.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// ac_events reads full payloads, as the ledger's diagnostic code lives in them.
pub(super) const EVENTS_PROFILE: ProjectionProfile = crate::events::PROFILE;
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
            // system: the daemon section says why.
            result.insert(
                "daemon".to_owned(),
                json!({
                    "online": false,
                    "error": {"code": error.code_spelling(), "message": error.message()},
                }),
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
        row.insert(
            "pause".to_owned(),
            json!({
                "revision": pause.revision,
                "stage": pause.stage,
                "pause_reason": pause.pause_reason,
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
        .map_err(|miss| ToolError::usage(miss.code(), miss.message(selector)))
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
        let document = read_bounded(&manifest, MAX_MANIFEST_BYTES).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| format!("{} is not JSON: {error}", manifest.display()))
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
            Err(reason) => warnings.push(json!({
                "code": ActingctlCode::InstallBuildManifestUnreadable.as_str(),
                "message": reason,
            })),
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

/// `v1.<server>.<connection>.<snapshot>.<after>.<query fingerprint>`: the Runtime cursor
/// bound to this server process and this connection, as ac_events reads it back.
pub(super) fn encode_cursor(
    context: &ToolContext<'_>,
    connected: &Connected,
    cursor: &RuntimeEventQueryCursor,
) -> Result<String, ToolError> {
    CursorBinding {
        session: context.session,
        generation: connected.generation,
    }
    .encode(cursor)
    .map_err(|error| {
        ToolError::new(
            "runtime",
            ActingctlCode::CursorEncodeFailed,
            error.message(),
        )
    })
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
            if arguments.has("offset") || arguments.has("max_bytes") {
                return Err(invalid_argument(
                    "offset",
                    "and max_bytes belong to mode text; export writes the whole material",
                ));
            }
            let connected = context.runtime.connect()?;
            let material = resolve_material(context, &connected, position, artifact_id)?;
            material_export(context, &connected, &material)
        }
        _ => Err(invalid_argument("mode", "must be text or export")),
    }
}

fn parse_material_ref(reference: &str) -> Result<(LedgerEventPosition, ArtifactId), ToolError> {
    let invalid = || {
        ToolError::usage(
            ActingctlCode::MaterialRefInvalid,
            "ref must be a materials[].ref from ac_events (<seq>:<event_id>:<artifact_id>)",
        )
    };
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
        ToolError::new(
            "runtime",
            ActingctlCode::MaterialQueryInvalid,
            format!("cannot form the event query: {error}"),
        )
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
            ToolError::usage(
                ActingctlCode::MaterialRefUnknown,
                "the ledger holds no such event and material at this sequence",
            )
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
        return Err(ToolError::usage(
            ActingctlCode::MaterialNotText,
            format!(
                "this material is {}; read it with mode export",
                text_of(json!(artifact.media_type))
            ),
        )
        .blocked_by("ac_material mode export"));
    }
    if offset >= artifact.byte_count {
        return Err(
            invalid_argument("offset", "is at or past the end of the material")
                .with_detail("total_bytes", json!(artifact.byte_count)),
        );
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
            .map_err(|_| invalid_argument("max_bytes", "is too large"))?,
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
    let text = utf8_prefix(&bytes).ok_or_else(|| {
        ToolError::usage(
            ActingctlCode::MaterialNotText,
            "the bytes at this offset are not UTF-8 text; start at 0 or at a next_offset, or use mode export",
        )
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
        return Err(ToolError::usage(
            ActingctlCode::MaterialTooLarge,
            format!(
                "the material has {} bytes; ac_material exports at most {MAX_EXPORT_BYTES}",
                artifact.byte_count
            ),
        ));
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
        RuntimeMaterialCompleteResult::NotProvided {
            limit,
            failure,
            error,
            ..
        } => Err(match error {
            Some(error) => context
                .runtime
                .failure(connected, &error)
                .with_detail("material_limit", json!(limit))
                .with_detail("material_failure", json!(failure)),
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
        } => Err(context
            .runtime
            .failure(connected, &error)
            .with_detail("material_state", json!(state))
            .with_detail("material_failure", json!(failure))),
    }
}

/// The Runtime answered the read without the material: its state, limit and typed failure
/// are passed on as they are. Without a failure the code is `material_<state>`, actingctl's own
/// dynamic family, which A5 splits into one fixed code with a `material_state` value.
fn material_unavailable(
    state: RuntimeMaterialReadState,
    limit: Option<RuntimeMaterialReadLimit>,
    failure: Option<&RuntimeMaterialReadFailure>,
) -> ToolError {
    let state_text = text_of(json!(state));
    let relayed_code = failure.map_or_else(
        || format!("material_{state_text}"),
        |failure| failure.code.clone(),
    );
    ToolError::coded(
        "runtime",
        ToolCode::Relayed { relayed_code },
        format!("the Runtime did not provide the material (state {state_text})"),
    )
    .with_detail("material_state", json!(state))
    .with_detail("material_limit", json!(limit))
    .with_detail("material_failure", json!(failure))
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
/// `{path, sha256, size}`.
pub(super) fn export_bytes(
    sha256: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Value, ToolError> {
    crate::material_export::write(&export_directory(), sha256, extension, bytes).map_err(
        |failure| {
            let error = ToolError::new(
                "runtime",
                ActingctlCode::MaterialExportFailed,
                failure.message(),
            )
            .with_detail("path", json!(failure.path().display().to_string()));
            match failure.partial() {
                Some(partial) => error.with_detail("partial", json!(partial.display().to_string())),
                None => error,
            }
        },
    )
}
