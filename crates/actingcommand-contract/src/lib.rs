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
    fn unsigned_integer_comma_grouping_is_explicit_and_canonical() {
        use serde_json::json;

        let source = json!({"type":"unsigned_integer","min":0,"max":u64::MAX});
        let declared: OcrFieldType = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(&declared).unwrap(), source);
        let OcrFieldType::UnsignedInteger { min, max, format } = declared else {
            panic!("unsigned integer declaration expected");
        };
        assert_eq!(format, OcrUnsignedIntegerFormat::AsciiDecimal);
        for (text, expected) in [("0", 0), ("00017", 17), ("18446744073709551615", u64::MAX)] {
            assert_eq!(format.parse(text, min, max), Ok(expected), "{text}");
        }
        for text in ["1,234", "1/2", "+1", "-1", "1.0", "１", " 1"] {
            assert_eq!(
                format.parse(text, min, max),
                Err(OcrFieldReason::InvalidInteger),
                "{text}"
            );
        }
        assert_eq!(format.parse("", min, max), Err(OcrFieldReason::Empty));
        assert_eq!(
            format.parse("18446744073709551616", min, max),
            Err(OcrFieldReason::Overflow)
        );

        let grouped_source = json!({
            "type":"unsigned_integer","min":0,"max":u64::MAX,"format":"comma_grouped"
        });
        let grouped: OcrFieldType = serde_json::from_value(grouped_source.clone()).unwrap();
        assert_eq!(serde_json::to_value(&grouped).unwrap(), grouped_source);
        let OcrFieldType::UnsignedInteger { format, .. } = grouped else {
            panic!("unsigned integer declaration expected");
        };
        for (text, expected) in [
            ("0", 0),
            ("17", 17),
            ("999", 999),
            ("1,000", 1000),
            ("1,234,567", 1_234_567),
            ("18,446,744,073,709,551,615", u64::MAX),
        ] {
            assert_eq!(format.parse(text, 0, u64::MAX), Ok(expected), "{text}");
        }
        for text in [
            "1000", "1,00", "12,34,567", "1,,000", ",123", "123,", "0,123", "01,234", "00",
            "1.234", "1 234", "+1,234", "１,234", "1,234x",
        ] {
            assert_eq!(
                format.parse(text, 0, u64::MAX),
                Err(OcrFieldReason::InvalidInteger),
                "{text}"
            );
        }
        assert_eq!(format.parse("", 0, u64::MAX), Err(OcrFieldReason::Empty));
        assert_eq!(
            format.parse("18,446,744,073,709,551,616", 0, u64::MAX),
            Err(OcrFieldReason::Overflow)
        );
        assert_eq!(format.parse("1,234", 1234, 1234), Ok(1234));
        assert_eq!(format.parse("1,234", 1235, 2000), Err(OcrFieldReason::OutOfRange));
        assert_eq!(format.parse("1,234", 0, 1233), Err(OcrFieldReason::OutOfRange));
        let mut unknown = grouped_source;
        unknown["format"] = json!("automatic");
        assert!(serde_json::from_value::<OcrFieldType>(unknown).is_err());
    }

    #[test]
    fn unsigned_integer_current_capacity_validates_both_complete_parts() {
        use serde_json::json;

        let source = json!({
            "type":"unsigned_integer","min":0,"max":u64::MAX,"format":"current_capacity"
        });
        let declared: OcrFieldType = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(&declared).unwrap(), source);
        let OcrFieldType::UnsignedInteger { format, .. } = declared else {
            panic!("unsigned integer declaration expected");
        };
        for (text, expected) in [
            ("17/20", 17),
            ("30/20", 30),
            ("0/0", 0),
            ("0007/0009", 7),
            ("18446744073709551615/0", u64::MAX),
            ("0/18446744073709551615", 0),
        ] {
            assert_eq!(format.parse(text, 0, u64::MAX), Ok(expected), "{text}");
            assert_eq!(
                OcrUnsignedIntegerFormat::AsciiDecimal.parse(text, 0, u64::MAX),
                Err(OcrFieldReason::InvalidInteger)
            );
        }
        for text in [
            "17", "/20", "17/", "17/20/1", "17 /20", "17/ 20", "+17/20", "17/-20", "17/20x",
            "17/2,000", "1,000/2000", "17/20.0", "17/２０",
        ] {
            assert_eq!(
                format.parse(text, 0, u64::MAX),
                Err(OcrFieldReason::InvalidInteger),
                "{text}"
            );
        }
        assert_eq!(format.parse("", 0, u64::MAX), Err(OcrFieldReason::Empty));
        for text in ["18446744073709551616/1", "1/18446744073709551616"] {
            assert_eq!(
                format.parse(text, 0, u64::MAX),
                Err(OcrFieldReason::Overflow),
                "{text}"
            );
        }
        assert_eq!(format.parse("17/999", 17, 17), Ok(17));
        assert_eq!(format.parse("17/20", 18, 30), Err(OcrFieldReason::OutOfRange));
        assert_eq!(format.parse("17/20", 0, 16), Err(OcrFieldReason::OutOfRange));
        let raw = " 17/20 ";
        assert_eq!(format.parse(raw.trim(), 0, u64::MAX), Ok(17));
        assert_eq!(raw, " 17/20 ");
    }

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
        declaration.fields[0].value = OcrFieldType::UnsignedInteger {
            min: 0,
            max: 99,
            format: OcrUnsignedIntegerFormat::default(),
        };
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
