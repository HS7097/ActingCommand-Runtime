// SPDX-License-Identifier: AGPL-3.0-only

//! `frames` (Workflow #434 LAB-1b; register §1.287), moved from actingctl's `stream`: a sequence
//! of frames of one instance captured by the Runtime, `CaptureSequence`, its flow answered as the
//! Runtime gives it.

use actingcommand_contract::CaptureSequenceSpec;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError, RuntimeFlowOutput};

/// `frames` defaults: how many frames, and the interval between two.
const DEFAULT_FRAME_COUNT: u16 = 1;
const DEFAULT_INTERVAL_MS: u64 = 250;

/// The capture of `count` frames (default `DEFAULT_FRAME_COUNT`) `interval_ms` apart (default
/// `DEFAULT_INTERVAL_MS`); `None` for a capture the contract refuses.
pub fn spec(count: Option<u16>, interval_ms: Option<u64>) -> Option<CaptureSequenceSpec> {
    CaptureSequenceSpec::new(
        count.unwrap_or(DEFAULT_FRAME_COUNT),
        interval_ms.unwrap_or(DEFAULT_INTERVAL_MS),
    )
    .ok()
}

/// `frames`: the capture `spec` of the instance `alias`.
pub fn capture(
    client: &RuntimeClient,
    alias: &str,
    spec: CaptureSequenceSpec,
) -> Result<RuntimeFlowOutput, RuntimeClientError> {
    client.capture_sequence(alias, spec)
}
