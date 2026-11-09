// SPDX-License-Identifier: AGPL-3.0-only

//! `material` (Workflow #434 LAB-1b; register §1.287), moved from MCP's `ac_material` (#338
//! §四): one committed material found again from its ref (`<seq>:<event_id>:<artifact_id>`, a
//! `materials[].ref` of `events`) with one `QueryEvents` page at its sequence, then read through
//! `ReadMaterial` and verified: a range of its text ([`read_text`]) or the whole material for an
//! export ([`read_whole`], which MCP's `ac_observe` shares). Where an export is written, the call
//! budget and the cancellation stay with the caller. Every failure is an outcome (Workflow #378
//! A5).

use crate::codes::{ActingctlCode, MaterialLimit, MaterialState};
use crate::material_export::MAX_EXPORT_BYTES;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ArtifactId, ArtifactMediaType, EventId, EventQuery, LedgerEventPosition,
    MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectedArtifactReference, ProjectionProfile,
    RuntimeEventQueryPageRequest, RuntimeMaterialReadFailure, RuntimeMaterialReadLimit,
    RuntimeMaterialReadRequest, RuntimeMaterialReadState,
};
use actingcommand_runtime_client::{
    RuntimeClient, RuntimeClientError, RuntimeMaterialCompleteResult, RuntimeMaterialSelection,
};
use serde_json::{Value, json};
use std::time::Instant;

/// The most text one read answers, and how much by default.
pub const MAX_TEXT_BYTES: u64 = 8192;
pub const DEFAULT_TEXT_BYTES: u64 = 4096;

/// One committed material, found again from its ref at the ledger's current snapshot (or, for
/// `ac_observe`, the frame its observation just recorded).
pub struct Material {
    pub position: LedgerEventPosition,
    pub artifact: ProjectedArtifactReference,
    pub snapshot: u64,
}

/// Why a material was not read.
pub enum Failure {
    /// A failure of the call's arguments: its outcome (`material_ref_invalid`,
    /// `material_ref_unknown`, `material_not_text` at an offset, `material_too_large`).
    Usage(Outcome),
    /// The material is no text: `material_not_text` with its `media_type`; read it whole.
    NotText(Outcome),
    /// An integer argument outside `minimum..=maximum`.
    OutOfRange {
        argument: &'static str,
        minimum: u64,
        maximum: u64,
    },
    /// The Runtime answered without the material, or the page request was refused: its outcome
    /// (`material_unavailable`, `material_query_invalid`).
    Unavailable(Outcome),
    /// The Runtime refused a request, or did not answer.
    Runtime(RuntimeClientError),
    /// A whole read the client's failure ended, with what the read reached
    /// (`material_unavailable`), its note.
    RuntimeNoted {
        error: RuntimeClientError,
        reached: Outcome,
    },
}

/// `<seq>:<event_id>:<artifact_id>`, a `materials[].ref` of `events`; anything else is
/// `material_ref_invalid`.
pub fn parse_ref(reference: &str) -> Result<(LedgerEventPosition, ArtifactId), Failure> {
    let invalid = || Failure::Usage(Outcome::new(ActingctlCode::MaterialRefInvalid));
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

/// The material `artifact_id` of the event at `position`, found with one `QueryEvents` page at
/// its sequence; one the ledger does not hold there is `material_ref_unknown` with the
/// `event_id` and `artifact_id`.
pub fn resolve(
    client: &RuntimeClient,
    position: LedgerEventPosition,
    artifact_id: ArtifactId,
) -> Result<Material, Failure> {
    let query = EventQuery {
        from_sequence: Some(position.sequence),
        to_sequence: Some(position.sequence),
        ..EventQuery::default()
    };
    let request = RuntimeEventQueryPageRequest::new(1, None).map_err(|error| {
        Failure::Unavailable(
            Outcome::new(ActingctlCode::MaterialQueryInvalid).caused_by(Outcome::new(error.code())),
        )
    })?;
    let page = client
        .query_event_page(query, ProjectionProfile::Concise, request)
        .map_err(Failure::Runtime)?;
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
            Failure::Usage(
                Outcome::new(ActingctlCode::MaterialRefUnknown)
                    .event_id(text_of(json!(position.event_id)))
                    .artifact_id(text_of(json!(artifact_id))),
            )
        })?;
    Ok(Material {
        position,
        artifact,
        snapshot: page.snapshot_ledger_position(),
    })
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// At most `max_bytes` of the text of `material` from `offset`: `{text, offset, bytes,
/// total_bytes, media_type, next_offset?}`, ending on a character boundary, made shorter until
/// `fits` takes it (escaped text can outgrow an answer budget). A material that is no JSON or
/// plain text is `material_not_text` with its media type; an offset at or past its end is out of
/// range; bytes at `offset` that are no UTF-8 text are `material_not_text` with the `offset`.
pub fn read_text(
    client: &RuntimeClient,
    material: &Material,
    offset: u64,
    max_bytes: u64,
    fits: &dyn Fn(&Value) -> bool,
) -> Result<Value, Failure> {
    let artifact = &material.artifact;
    if !matches!(
        artifact.media_type,
        ArtifactMediaType::ApplicationJson | ArtifactMediaType::TextPlain
    ) {
        // Read it whole; its media type travels as `media_type`.
        return Err(Failure::NotText(
            Outcome::new(ActingctlCode::MaterialNotText)
                .media_type(text_of(json!(artifact.media_type))),
        ));
    }
    if offset >= artifact.byte_count {
        // At or past the end: the offsets this material has.
        return Err(Failure::OutOfRange {
            argument: "offset",
            minimum: 0,
            maximum: artifact.byte_count.saturating_sub(1),
        });
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
        requested_length: u32::try_from(length).map_err(|_| Failure::OutOfRange {
            argument: "max_bytes",
            minimum: 1,
            maximum: MAX_TEXT_BYTES,
        })?,
        max_reply_bytes: MAX_RUNTIME_MATERIAL_REPLY_BYTES,
    };
    let read = client.read_material(request).map_err(Failure::Runtime)?;
    let bytes = match (read.state, read.chunk) {
        (RuntimeMaterialReadState::Verified, Some(chunk)) => chunk.bytes,
        (state, _) => {
            return Err(Failure::Unavailable(unavailable_outcome(
                state,
                read.limit,
                read.failure.as_ref(),
            )));
        }
    };
    // The bytes at this offset are not UTF-8 text: start at 0 or at a next_offset, or read it
    // whole.
    let text = utf8_prefix(&bytes).ok_or_else(|| {
        Failure::Usage(Outcome::new(ActingctlCode::MaterialNotText).offset(offset))
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
        if end == 0 || fits(&result) {
            return Ok(result);
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

/// The whole of `material`, read through `ReadMaterial` until `deadline` while `keep_reading`
/// says so, and verified against its reference. One larger than `MAX_EXPORT_BYTES` is
/// `material_too_large`. A client failure during the read keeps the client's failure, with what
/// the read reached as its note.
pub fn read_whole(
    client: &RuntimeClient,
    material: &Material,
    deadline: Instant,
    keep_reading: &dyn Fn() -> bool,
) -> Result<Vec<u8>, Failure> {
    let artifact = &material.artifact;
    if artifact.byte_count > MAX_EXPORT_BYTES as u64 {
        return Err(Failure::Usage(
            Outcome::new(ActingctlCode::MaterialTooLarge)
                .actual_bytes(artifact.byte_count)
                .limit_bytes(MAX_EXPORT_BYTES),
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
    match client.read_material_complete(selection, MAX_EXPORT_BYTES, deadline, keep_reading) {
        RuntimeMaterialCompleteResult::Verified { bytes, .. } => Ok(bytes),
        RuntimeMaterialCompleteResult::NotProvided {
            limit,
            failure,
            error,
            ..
        } => Err(match error {
            Some(error) => Failure::RuntimeNoted {
                error,
                reached: unavailable_outcome(
                    RuntimeMaterialReadState::NotProvided,
                    Some(limit),
                    failure.as_ref(),
                ),
            },
            None => Failure::Unavailable(unavailable_outcome(
                RuntimeMaterialReadState::NotProvided,
                Some(limit),
                failure.as_ref(),
            )),
        }),
        RuntimeMaterialCompleteResult::Failed {
            state,
            failure,
            error,
            ..
        } => Err(Failure::RuntimeNoted {
            error,
            reached: unavailable_outcome(state, None, failure.as_ref()),
        }),
    }
}

/// `material_unavailable` (Workflow #378 A5): the read's state (`material_state`) and, when the
/// read stopped short, its limit (`material_limit`), caused by the Runtime's typed failure code
/// when it gave one (relayed as the Runtime spelled it).
pub fn unavailable_outcome(
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
