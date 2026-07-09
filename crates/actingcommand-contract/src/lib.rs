// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline contract definitions for ActingCommand runtime boundaries.
//!
//! These models define the Rust-side API vocabulary. They are skeleton
//! contracts for protocol, device, and engine boundaries, not game logic.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

pub mod game_engine;
pub mod lab;
pub mod primitive;
pub mod taskflow;
pub mod types;

pub use game_engine::*;
pub use lab::*;
pub use primitive::*;
pub use taskflow::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_key_constants_keep_backend_variant_names() {
        assert_eq!(SERVER_ALAS_JP, "alas.jp");
        assert_eq!(SERVER_BAAS_GLOBAL_EN, "baas.global_en");
        assert_eq!(SERVER_MAA_YOSTAR_JP, "maa.yostar_jp");
    }

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
