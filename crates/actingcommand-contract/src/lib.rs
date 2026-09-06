// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline contract definitions for ActingCommand runtime boundaries.
//!
//! These models define the Rust-side API vocabulary. They are skeleton
//! contracts for protocol, device, and engine boundaries, not game logic.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

pub mod agent;
pub mod event;
pub mod fact;
pub mod game_engine;
pub mod interaction;
pub mod lab;
pub mod monitor;
pub mod page_projection;
pub mod performance;
pub mod primitive;
pub mod project;
pub mod proposal;
pub mod runtime;
pub mod state;
pub mod taskflow;
pub mod types;

pub use agent::*;
pub use event::*;
pub use fact::*;
pub use game_engine::*;
pub use interaction::*;
pub use lab::*;
pub use monitor::*;
pub use performance::*;
pub use primitive::*;
pub use project::*;
pub use proposal::*;
pub use runtime::*;
pub use state::*;
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
    fn declared_suffix_extraction_preserves_exact_dictionary_input() {
        use serde_json::json;
        let rule_json = json!({"mode":"strip_declared_suffix_v1","suffix":[
            {"type":"ascii_digits","count":2}, {"type":"literal","value":"/"},
            {"type":"ascii_digits","count":4}, {"type":"literal","value":":"},
            {"type":"ascii_digits","count":2}, {"type":"literal","value":"期限"}
        ]});
        let rule: OcrFieldTextExtraction = serde_json::from_value(rule_json.clone()).unwrap();
        let original = "  Échantillon Ω 09/2803:59期限  ";
        let normalized = original.trim();
        let evidence = rule.extract(normalized).unwrap().unwrap();
        assert_eq!(evidence.extracted_text, "Échantillon Ω");
        assert_eq!(
            &normalized[evidence.matched_suffix.start..evidence.matched_suffix.end],
            "09/2803:59期限"
        );
        assert_eq!(
            &normalized[evidence.extracted_range.start..evidence.extracted_range.end],
            "Échantillon Ω"
        );
        assert_eq!(
            evidence.rule_version,
            OcrFieldExtractionVersion::StripDeclaredSuffixV1
        );
        rule.verify(normalized, &evidence).unwrap();
        assert_eq!(normalized, "Échantillon Ω 09/2803:59期限");
        assert_eq!(original, "  Échantillon Ω 09/2803:59期限  ");
        for text in [
            "Échantillon Ω",
            "Échantillon Ω09/2803:59other",
            "Échantillon Ω０9/2803:59期限",
            "Échantillon Ω09/2803:59期限 extra",
            "09/2803:59期限",
            " 09/2803:59期限",
            "期限",
        ] {
            assert!(rule.extract(text).unwrap().is_none(), "{text}");
        }
        for case in 0..6 {
            let mut forged = evidence.clone();
            match case {
                0 => forged.matched_suffix.start = 1,
                1 => forged.matched_suffix.end = normalized.len() + 1,
                2 => forged.extracted_range.start = 1,
                3 => forged.extracted_range.end = usize::MAX,
                4 => forged.extracted_text = "Other".into(),
                _ => forged.extracted_range.start = forged.extracted_range.end + 1,
            }
            assert!(rule.verify(normalized, &forged).is_err());
        }
        let mut unknown_version = serde_json::to_value(&evidence).unwrap();
        unknown_version["rule_version"] = json!("unknown");
        assert!(serde_json::from_value::<OcrFieldExtractionEvidence>(unknown_version).is_err());
        let other_rule: OcrFieldTextExtraction = serde_json::from_value(json!({
            "mode":"strip_declared_suffix_v1","suffix":[{"type":"literal","value":"期限"}]
        }))
        .unwrap();
        assert!(other_rule.verify(normalized, &evidence).is_err());

        let declaration_json = json!({"mode":"fields_v1","page_ids":["panel"],"fields":[{
            "id":"name","group":"item","target_id":"name","required":true,"privacy":"public",
            "trim":"whitespace_v1","value":{"type":"dictionary_entry","dictionary":{"path":"words.json","sha256":"a".repeat(64)}}
        }],"limits":{"max_frames":1,"max_items":2,"max_string_bytes":128,"max_total_bytes":4096,"max_truth_entries":16},"outcome_key":"fields_recorded"});
        let mut declaration: OcrFieldsDeclaration =
            serde_json::from_value(declaration_json.clone()).unwrap();
        declaration.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&declaration).unwrap(),
            declaration_json
        );
        assert!(declaration.fields[0].text_extraction.is_none());
        declaration.fields[0].text_extraction = Some(rule.clone());
        declaration.validate().unwrap();
        let roundtrip: OcrFieldsDeclaration =
            serde_json::from_value(serde_json::to_value(&declaration).unwrap()).unwrap();
        assert_eq!(roundtrip, declaration);
        declaration.fields[0].value = OcrFieldType::UnsignedInteger { min: 0, max: 99 };
        assert_eq!(
            declaration.validate(),
            Err("ocr_fields_text_extraction_type_invalid")
        );

        for invalid in [
            json!({"mode":"unknown","suffix":[]}),
            json!({"mode":"strip_declared_suffix_v1","suffix":[{"type":"unknown"}]}),
            json!({"mode":"strip_declared_suffix_v1","suffix":[],"extra":true}),
        ] {
            assert!(serde_json::from_value::<OcrFieldTextExtraction>(invalid).is_err());
        }
        for suffix in [
            json!([]),
            json!([{"type":"ascii_digits","count":0}]),
            json!([{"type":"ascii_digits","count":9}]),
            json!([{"type":"literal","value":""}]),
            json!([{"type":"literal","value":"界".repeat(22)}]),
            json!(vec![json!({"type":"literal","value":"x"}); 17]),
            json!(vec![json!({"type":"literal","value":"x".repeat(64)}); 5]),
        ] {
            let invalid: OcrFieldTextExtraction =
                serde_json::from_value(json!({"mode":"strip_declared_suffix_v1","suffix":suffix}))
                    .unwrap();
            assert!(invalid.validate().is_err());
            assert!(invalid.extract(normalized).is_err());
        }
        for suffix in [
            json!([{"type":"ascii_digits","count":1}]),
            json!([{"type":"ascii_digits","count":8}]),
            json!(vec![json!({"type":"literal","value":"x"}); 16]),
            json!(vec![json!({"type":"literal","value":"x".repeat(64)}); 4]),
        ] {
            let valid: OcrFieldTextExtraction =
                serde_json::from_value(json!({"mode":"strip_declared_suffix_v1","suffix":suffix}))
                    .unwrap();
            valid.validate().unwrap();
        }
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
