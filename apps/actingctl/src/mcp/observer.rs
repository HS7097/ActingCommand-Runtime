// SPDX-License-Identifier: AGPL-3.0-only

//! Observer tools (#338 §四 工具表): ac_overview, ac_events and ac_material. They only
//! read; like the CLI's reads they record no client.action. Every fact comes from the
//! Runtime and runtime-client unchanged; this module only selects, pages and bounds it.

use super::runs;
use super::runtime::{Connected, Location, read_bounded};
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use actingcommand_contract::{
    ArtifactId, ArtifactMediaType, EventId, EventQuery, LedgerEventPosition, LedgerView,
    MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectedArtifactReference, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, RuntimeEventQueryCursor, RuntimeEventQueryPageRequest,
    RuntimeInstanceStatus, RuntimeMaterialReadFailure, RuntimeMaterialReadLimit,
    RuntimeMaterialReadRequest, RuntimeMaterialReadState, RuntimeMonitorInstanceStatus,
};
use actingcommand_runtime_client::{RuntimeMaterialCompleteResult, RuntimeMaterialSelection};
use serde_json::{Map, Value, json};
use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// `<root>\runtime\BUILD-MANIFEST.json` is read whole up to this size.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// ac_events reads full payloads, as the ledger's diagnostic code lives in them.
pub(super) const EVENTS_PROFILE: ProjectionProfile = ProjectionProfile::Forensic;
const MAX_TEXT_BYTES: u64 = 8192;
const DEFAULT_TEXT_BYTES: u64 = 4096;
/// The largest material ac_material exports.
const MAX_EXPORT_BYTES: usize = 32 * 1024 * 1024;
/// Time kept back from the call budget for writing the exported file.
const EXPORT_WRITE_RESERVE: Duration = Duration::from_secs(2);

static EXPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn overview(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance"])?;
    let selector = arguments.string("instance")?;
    let location = context.runtime.locate();
    let mut warnings = Vec::new();
    let install = install_view(&location, &mut warnings);
    let lab_present = location
        .root
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
                    "error": {"code": error.code(), "message": error.message()},
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
                "reason_code": pause.reason_code,
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
/// Runtime's status lists them.
pub(super) fn select_instance<'s>(
    instances: &'s [RuntimeInstanceStatus],
    selector: &str,
) -> Result<&'s RuntimeInstanceStatus, ToolError> {
    let port = selector.parse::<u16>().ok();
    let mut matching = instances.iter().filter(|instance| {
        instance.instance_alias() == selector
            || json!(instance.instance_id()).as_str() == Some(selector)
            || port.is_some_and(|port| instance.adb_port() == Some(port))
    });
    let selected = matching.next().ok_or_else(|| {
        ToolError::usage(
            "instance_unknown",
            format!("no configured instance has the alias, instance_id or ADB port {selector}"),
        )
    })?;
    if matching.next().is_some() {
        return Err(ToolError::usage(
            "instance_ambiguous",
            format!("{selector} names more than one instance; pass its alias or instance_id"),
        ));
    }
    Ok(selected)
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
                "code": "install_build_manifest_unreadable",
                "message": reason,
            })),
        }
    }
    Value::Object(install)
}

pub(super) fn events(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(
        arguments,
        &["instance", "view", "since_unix_ms", "cursor", "limit"],
    )?;
    let selector = arguments.string("instance")?;
    let view = arguments.string("view")?.map(parse_view).transpose()?;
    let since_unix_ms = arguments.integer("since_unix_ms", 0, u64::MAX)?;
    let cursor = arguments.string("cursor")?;
    let limit = arguments
        .integer("limit", 1, tools::MAX_LIST)?
        .unwrap_or(tools::DEFAULT_LIST);
    let limit = u16::try_from(limit).map_err(|_| invalid_argument("limit", "is too large"))?;
    let connected = context.runtime.connect()?;
    let instance_id = match selector {
        Some(selector) => {
            let status = connected
                .client
                .status()
                .map_err(|error| context.runtime.failure(&connected, &error))?;
            Some(select_instance(status.instances(), selector)?.instance_id())
        }
        None => None,
    };
    let query = EventQuery {
        view,
        from_timestamp_unix_ms: since_unix_ms,
        instance_id,
        ..EventQuery::default()
    };
    let resume = cursor
        .map(|token| decode_cursor(context, &connected, token, &query))
        .transpose()?;
    let request = RuntimeEventQueryPageRequest::new(limit, resume).map_err(|_| cursor_invalid())?;
    let page = connected
        .client
        .query_event_page(query.clone(), EVENTS_PROFILE, request)
        .map_err(|error| context.runtime.failure(&connected, &error))?;
    let snapshot = page.snapshot_ledger_position();
    let mut rows = page.events().iter().map(event_row).collect::<Vec<_>>();
    let mut kept = rows.len();
    loop {
        let truncated = kept < rows.len();
        let next = if truncated {
            let after = page.events()[kept - 1].sequence;
            Some(
                RuntimeEventQueryCursor::new(snapshot, after, &query, EVENTS_PROFILE).map_err(
                    |error| {
                        ToolError::new(
                            "runtime",
                            "cursor_encode_failed",
                            format!("cannot form the next cursor: {error}"),
                        )
                    },
                )?,
            )
        } else {
            page.next_cursor().cloned()
        };
        let next = next
            .map(|cursor| encode_cursor(context, &connected, &cursor))
            .transpose()?;
        let result = page_result(&rows[..kept], next, truncated, snapshot);
        if tools::fits_success(&result) {
            return Ok(ToolSuccess::new(result));
        }
        if kept > 1 {
            kept -= 1;
            continue;
        }
        // One row alone is over the budget: shorten its material list, and say so, rather
        // than drop the row.
        let Some(materials) = rows
            .first_mut()
            .and_then(|row| row.get_mut("materials"))
            .and_then(Value::as_array_mut)
            .filter(|materials| !materials.is_empty())
        else {
            // Nothing left to shorten: the result answers output_budget_exceeded.
            return Ok(ToolSuccess::new(result));
        };
        materials.pop();
        rows[0]["materials_truncated"] = json!(true);
    }
}

fn parse_view(view: &str) -> Result<LedgerView, ToolError> {
    serde_json::from_value(json!(view))
        .map_err(|_| invalid_argument("view", "is not a ledger view"))
}

fn page_result(
    rows: &[Value],
    next_cursor: Option<String>,
    truncated: bool,
    snapshot: u64,
) -> Value {
    let mut result = Map::new();
    result.insert("events".to_owned(), Value::Array(rows.to_vec()));
    if let Some(cursor) = next_cursor {
        result.insert("next_cursor".to_owned(), json!(cursor));
    }
    if truncated {
        result.insert("truncated".to_owned(), json!(true));
    }
    result.insert("snapshot_ledger_position".to_owned(), json!(snapshot));
    Value::Object(result)
}

pub(super) fn event_row(event: &ProjectedEvent) -> Value {
    let mut row = Map::new();
    row.insert("seq".to_owned(), json!(event.sequence));
    row.insert("ts".to_owned(), json!(event.timestamp_unix_ms));
    row.insert("type".to_owned(), json!(event.event_type));
    row.insert("severity".to_owned(), json!(event.severity));
    if let Some(instance_id) = event.links.instance_id() {
        row.insert("instance".to_owned(), json!(instance_id));
    }
    if let Some(request_id) = event.links.request_id() {
        row.insert("request_id".to_owned(), json!(request_id));
    }
    if let Some(run_id) = event.links.run_id() {
        row.insert("run_id".to_owned(), json!(run_id));
    }
    let code = match &event.payload {
        ProjectionPayload::Full(payload) => payload.diagnostic_code(),
        _ => None,
    };
    if let Some(code) = code {
        row.insert("code".to_owned(), json!(code.as_str()));
    }
    if !event.artifacts.is_empty() {
        let materials = event
            .artifacts
            .iter()
            .map(|artifact| {
                json!({
                    "ref": material_ref(event, artifact),
                    "media_type": artifact.media_type,
                    "size": artifact.byte_count,
                })
            })
            .collect();
        row.insert("materials".to_owned(), Value::Array(materials));
    }
    Value::Object(row)
}

/// `<seq>:<event_id>:<artifact_id>`: enough to find the material again in the ledger.
fn material_ref(event: &ProjectedEvent, artifact: &ProjectedArtifactReference) -> String {
    format!(
        "{}:{}:{}",
        event.sequence,
        text_of(json!(event.event_id)),
        text_of(json!(artifact.artifact_id))
    )
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn cursor_invalid() -> ToolError {
    ToolError::usage(
        "cursor_invalid",
        "this cursor belongs to another query, Runtime connection or server process; read again from the first page without a cursor",
    )
}

/// `v1.<server>.<connection>.<snapshot>.<after>.<query fingerprint>`: the Runtime cursor
/// bound to this server process and this connection.
pub(super) fn encode_cursor(
    context: &ToolContext<'_>,
    connected: &Connected,
    cursor: &RuntimeEventQueryCursor,
) -> Result<String, ToolError> {
    let fingerprint = json!(cursor)
        .get("query_fingerprint")
        .and_then(Value::as_str)
        .and_then(|fingerprint| fingerprint.strip_prefix("sha256:"))
        .map(str::to_owned)
        .ok_or_else(|| {
            ToolError::new(
                "runtime",
                "cursor_encode_failed",
                "the Runtime cursor carries no query fingerprint",
            )
        })?;
    Ok(format!(
        "v1.{:016x}.{}.{}.{}.{fingerprint}",
        context.session,
        connected.generation,
        cursor.snapshot_ledger_position(),
        cursor.after_sequence()
    ))
}

fn decode_cursor(
    context: &ToolContext<'_>,
    connected: &Connected,
    token: &str,
    query: &EventQuery,
) -> Result<RuntimeEventQueryCursor, ToolError> {
    let parts = token.split('.').collect::<Vec<_>>();
    let [version, session, generation, snapshot, after, fingerprint] = parts.as_slice() else {
        return Err(cursor_invalid());
    };
    if *version != "v1"
        || u64::from_str_radix(session, 16).ok() != Some(context.session)
        || generation.parse::<u64>().ok() != Some(connected.generation)
    {
        return Err(cursor_invalid());
    }
    let snapshot = snapshot.parse::<u64>().map_err(|_| cursor_invalid())?;
    let after = after.parse::<u64>().map_err(|_| cursor_invalid())?;
    let cursor = serde_json::from_value::<RuntimeEventQueryCursor>(json!({
        "snapshot_ledger_position": snapshot,
        "after_sequence": after,
        "query_fingerprint": format!("sha256:{fingerprint}"),
    }))
    .map_err(|_| cursor_invalid())?;
    if cursor.validate().is_err() || !cursor.matches(query, EVENTS_PROFILE).unwrap_or(false) {
        return Err(cursor_invalid());
    }
    Ok(cursor)
}

/// One committed material, found again from its ref at the ledger's current snapshot.
struct Material {
    position: LedgerEventPosition,
    artifact: ProjectedArtifactReference,
    snapshot: u64,
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
            "material_ref_invalid",
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
            "material_query_invalid",
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
                "material_ref_unknown",
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
            "material_not_text",
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
            "material_not_text",
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

fn material_export(
    context: &ToolContext<'_>,
    connected: &Connected,
    material: &Material,
) -> ToolOutcome {
    let artifact = &material.artifact;
    if artifact.byte_count > MAX_EXPORT_BYTES as u64 {
        return Err(ToolError::usage(
            "material_too_large",
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
/// are passed on as they are.
fn material_unavailable(
    state: RuntimeMaterialReadState,
    limit: Option<RuntimeMaterialReadLimit>,
    failure: Option<&RuntimeMaterialReadFailure>,
) -> ToolError {
    let state_text = text_of(json!(state));
    let code = failure.map_or_else(
        || format!("material_{state_text}"),
        |failure| failure.code.clone(),
    );
    ToolError::new(
        "runtime",
        code,
        format!("the Runtime did not provide the material (state {state_text})"),
    )
    .with_detail("material_state", json!(state))
    .with_detail("material_limit", json!(limit))
    .with_detail("material_failure", json!(failure))
}

/// Writes the verified bytes to `%TEMP%\actingcommand-mcp\materials\<sha256>.<ext>`
/// through a partial file moved into place.
fn write_export(artifact: &ProjectedArtifactReference, bytes: &[u8]) -> ToolOutcome {
    let failed = |action: &str, path: &Path, error: io::Error| {
        ToolError::new(
            "runtime",
            "material_export_failed",
            format!("{action} {}: {error}", path.display()),
        )
        .with_detail("path", json!(path.display().to_string()))
    };
    let digest = artifact
        .sha256
        .strip_prefix("sha256:")
        .unwrap_or(&artifact.sha256);
    let extension = artifact.kind.extension();
    let directory = env::temp_dir().join("actingcommand-mcp").join("materials");
    fs::create_dir_all(&directory)
        .map_err(|error| failed("cannot create", directory.as_path(), error))?;
    let path = directory.join(format!("{digest}.{extension}"));
    let partial = directory.join(format!(
        "{digest}.{extension}.{}-{}.partial",
        process::id(),
        EXPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&partial, bytes).map_err(|error| failed("cannot write", partial.as_path(), error))?;
    fs::rename(&partial, &path).map_err(|error| {
        failed("cannot move into place", path.as_path(), error)
            .with_detail("partial", json!(partial.display().to_string()))
    })?;
    Ok(ToolSuccess::new(json!({
        "path": path.display().to_string(),
        "sha256": artifact.sha256,
        "size": bytes.len(),
    })))
}
