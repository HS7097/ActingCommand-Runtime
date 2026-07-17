// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline contract definitions for ActingCommand runtime boundaries.
//!
//! These models define the Rust-side API vocabulary. They are skeleton
//! contracts for protocol, device, and engine boundaries, not game logic.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

pub mod event;
pub mod game_engine;
pub mod lab;
pub mod monitor;
pub mod primitive;
pub mod runtime;
pub mod taskflow;
pub mod types;

pub use event::*;
pub use game_engine::*;
pub use lab::*;
pub use monitor::*;
pub use primitive::*;
pub use runtime::*;
pub use taskflow::*;
pub use types::{
    AcquisitionCapture, ContractResult, DurationMillis, ENGINE_DELEGATED, ENGINE_NATIVE,
    EngineKind, GameKey, LogEvent, Metadata, ProfileId, ProfileSummary, RUNTIME_DEGRADED,
    RUNTIME_FATAL, RUNTIME_RUNNING, RUNTIME_STARTING, RUNTIME_STOPPED, RUNTIME_STOPPING,
    RUNTIME_UNKNOWN, Resolution, Resource, ResourceHistoryPoint, ResourceKey, RuntimeCapability,
    RuntimeContext, RuntimeError, RuntimeState, RuntimeStatus, SEVERITY_DEGRADED, SEVERITY_ERROR,
    SEVERITY_FATAL, SEVERITY_INFO, SEVERITY_WARNING, SchedulerSummary, ServerKey, Severity,
    TaskRunId, Timestamp,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_error_can_be_used_as_contract_error() {
        let err = RuntimeError {
            severity: SEVERITY_FATAL.to_string(),
            code: "invalid_contract".to_string(),
            message: "invalid primitive response".to_string(),
            module: "contract-test".to_string(),
            original_error: None,
            fallback_path: None,
            user_visible_impact: Some("request failed".to_string()),
            context: Metadata::new(),
            occurred_at: "2026-06-18T00:00:00Z".to_string(),
        };

        let result: ContractResult<()> = Err(err);
        assert!(result.is_err());
    }
}
