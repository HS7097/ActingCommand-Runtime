// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{CaptureBackend, create_capture_backend};
use actingcommand_lab::{CaptureBackendRequest, LabError};

/// Temporary direct-capture bridge for drive/run until C5 Task 5 moves those flows to Runtime.
pub(super) fn open_legacy_control_capture(
    request: CaptureBackendRequest,
) -> Result<Box<dyn CaptureBackend>, LabError> {
    let selected = create_capture_backend(request.config)
        .map_err(|error| LabError::device(error.to_string()))?;
    if let Some(observation) = request.observation {
        observation.record(actingcommand_lab::CaptureBackendReport {
            requested: selected.diagnostics.requested,
            used: selected.diagnostics.used,
            attempts: selected.diagnostics.attempts.clone(),
        })?;
    }
    Ok(selected.backend)
}
