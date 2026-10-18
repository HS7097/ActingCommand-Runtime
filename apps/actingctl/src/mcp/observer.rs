// SPDX-License-Identifier: AGPL-3.0-only

//! Observer tools (#338 §四 工具表): ac_overview and ac_material. They only read; like the
//! CLI's reads they record no client.action. Every fact comes from the Runtime and
//! runtime-client unchanged; this module only selects, pages and bounds it. ac_events lives in
//! `events` (Workflow #389 MCP-1). Errors and warnings are outcomes (Workflow #378 A5).
//! ac_overview is `actingctl overview` (`crate::overview`, Workflow #389 MCP-8): the same read
//! and result, cut here to the output budget in the order of the MCP-0 command table §4.9.

use super::runtime::{self, Connected};
use super::tools::{
    self, Arguments, McpBlocker, ToolContext, ToolError, ToolOutcome, ToolSuccess,
    invalid_argument, out_of_range,
};
use crate::codes::{ActingctlCode, ArgumentProblem, MaterialLimit, MaterialState};
use crate::overview::{self, Failure, OverviewRequest, Place, Sections, Warning};
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_contract::{
    ArtifactId, ArtifactMediaType, EventId, EventQuery, LedgerEventPosition,
    MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectedArtifactReference, ProjectionProfile,
    RuntimeEventQueryPageRequest, RuntimeInstanceStatus, RuntimeMaterialReadFailure,
    RuntimeMaterialReadLimit, RuntimeMaterialReadRequest, RuntimeMaterialReadState,
};
use actingcommand_runtime_client::{RuntimeMaterialCompleteResult, RuntimeMaterialSelection};
use serde_json::{Map, Value, json};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// MCP-0 command table §4.9: the top-level blocks ac_overview cuts first, in this order, when its
/// answer does not fit the output budget. A slice that adds an overview block registers its place
/// here in the same PR; a block the answer does not hold is passed over.
const CUT_BLOCKS: [&str; 9] = [
    "fact_refresh",
    "host",
    "startup",
    "tagged_state",
    "ledger",
    "performance",
    "watchdog",
    "catalog",
    "config_generation",
];
/// Then each instance row's `run`, from the last row back, then each row's `monitor`, the same
/// way, then each row's `foreground` (Workflow #389 MCP-5b registers it last of the row blocks: it
/// is the smallest); then `install.build`.
const CUT_ROW_BLOCKS: [&str; 3] = ["run", "monitor", "foreground"];
const MAX_TEXT_BYTES: u64 = 8192;
const DEFAULT_TEXT_BYTES: u64 = 4096;
/// The largest material ac_material exports; ac_observe and `actingctl observe --export` share it.
const MAX_EXPORT_BYTES: usize = crate::material_export::MAX_EXPORT_BYTES;
/// Time kept back from the call budget for writing the exported file.
const EXPORT_WRITE_RESERVE: Duration = Duration::from_secs(2);

/// ac_overview: `actingctl overview` over MCP (Workflow #389 MCP-8). Its arguments are the CLI's
/// flags (R2; `sections` a list where the CLI takes a comma-separated value); the server's
/// location stands for the CLI's `--root` and `--state-root`. The result is the CLI's stdout JSON
/// without its `warnings`, which go beside it, cut to the output budget (`cut_to_fit`).
pub(super) fn overview(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "sections"])?;
    let instance = arguments.string("instance")?.map(str::to_owned);
    let sections = match arguments.value("sections") {
        None => Sections::ALL,
        Some(Value::Array(names)) if names.is_empty() => {
            return Err(invalid_argument("sections", ArgumentProblem::ValueMissing));
        }
        Some(Value::Array(names)) => {
            let names = names
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or_else(|| invalid_argument("sections", ArgumentProblem::WrongType))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Sections::parse(names)
                .ok_or_else(|| invalid_argument("sections", ArgumentProblem::NotRecognised))?
        }
        Some(_) => return Err(invalid_argument("sections", ArgumentProblem::WrongType)),
    };
    let request = OverviewRequest { instance, sections };
    let location = context.runtime.locate();
    let place = Place {
        root: location.root.as_deref(),
        install_root: location.install_root.as_deref(),
        state_root: location.state_root.as_deref().ok(),
    };
    let connected = context.runtime.connect();
    // No connection: the daemon block says why, as the tool error `{class, outcome}`.
    let client = match &connected {
        Ok(connected) => Ok(&connected.client),
        Err(error) => Err(error.to_value()),
    };
    let answer = overview::read(client, &request, &place, context.deadline, &tools::envelope)
        .map_err(|failure| match &failure {
            Failure::Argument { argument, problem } => invalid_argument(argument, *problem),
            Failure::Selector { .. } => ToolError::outcome("usage", failure.outcome()),
            Failure::Install(_) | Failure::Clock => {
                ToolError::outcome("runtime", failure.outcome())
            }
            Failure::Runtime(error) => runtime_failure(context, connected.as_ref().ok(), error),
        })?;
    let warnings = answer
        .warnings
        .into_iter()
        .map(|warning| match warning {
            Warning::Outcome(outcome) => tools::warning(&outcome),
            Warning::Runtime(error) => {
                runtime_failure(context, connected.as_ref().ok(), &error).into_warning()
            }
        })
        .collect::<Vec<_>>();
    let mut result = Value::Object(answer.result);
    // The envelope is `{ok, result, warnings}`; this wrapper is a little larger, so an answer it
    // lets through always fits.
    cut_to_fit(&mut result, |result| {
        tools::fits_success(&json!({"result": result, "warnings": warnings}))
    });
    Ok(ToolSuccess { result, warnings })
}

/// A Runtime read failure as a tool error: on the server's connection, which is dropped when the
/// client locked it.
fn runtime_failure(
    context: &ToolContext<'_>,
    connected: Option<&Connected>,
    error: &actingcommand_runtime_client::RuntimeClientError,
) -> ToolError {
    match connected {
        Some(connected) => context.runtime.failure(connected, error),
        None => runtime::client_error(error),
    }
}

/// Cuts `result`, an ac_overview answer, one block at a time in the order of the MCP-0 command
/// table §4.9 until `fits` takes it: the top-level blocks of [`CUT_BLOCKS`], then each instance
/// row's `run` from the last row back, then each row's `monitor` the same way, then each row's
/// `foreground` (MCP-5b), then `install.build`. Each cut block leaves `<block>_truncated: true` where it was. The daemon, the
/// global pause, each row's identity, `emulator_state` (Workflow #390 九处-1),
/// `backend_selfcheck` (#411 A7), `pause`, `lease_active` and `queued`, and `incomplete` are
/// never cut: an answer that still does not fit when nothing is left to cut is returned as it is,
/// and the call answers `output_budget_exceeded` (`tools::call_result`).
pub(super) fn cut_to_fit(result: &mut Value, fits: impl Fn(&Value) -> bool) {
    while !fits(result) {
        if !cut_next(result) {
            return;
        }
    }
}

/// The next cut of [`cut_to_fit`]; false when nothing is left to cut.
fn cut_next(result: &mut Value) -> bool {
    let Some(members) = result.as_object_mut() else {
        return false;
    };
    for block in CUT_BLOCKS {
        if members.remove(block).is_some() {
            members.insert(format!("{block}_truncated"), json!(true));
            return true;
        }
    }
    for block in CUT_ROW_BLOCKS {
        let rows = members
            .get_mut(overview::Section::Instances.name())
            .and_then(Value::as_array_mut);
        for row in rows.into_iter().flatten().rev() {
            if let Some(row) = row.as_object_mut()
                && row.remove(block).is_some()
            {
                row.insert(format!("{block}_truncated"), json!(true));
                return true;
            }
        }
    }
    if let Some(install) = members
        .get_mut(overview::Section::Install.name())
        .and_then(Value::as_object_mut)
        && install.remove("build").is_some()
    {
        install.insert("build_truncated".to_owned(), json!(true));
        return true;
    }
    false
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
