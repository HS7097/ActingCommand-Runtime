// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use std::sync::{Mutex, Once};

crate::outcome_codes! {
    enum TestCode {
        Failed => "test_operation_failed": error,
        Unusable => "test_store_unusable": fatal,
        Done => "test_operation_done": success,
    }
}

crate::outcome_locations! {
    enum TestLocation {
        ReadIndex => "test_read_index",
    }
}

static SINK_TEXTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn recording_sink() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        install_diagnostic_sink(Box::new(|outcome: &Outcome, text: &str| {
            SINK_TEXTS.lock().expect("lock the test sink").push(format!(
                "{} {}",
                outcome.code(),
                text.len()
            ));
        }))
        .expect("install the test sink");
    });
}

#[test]
fn a_full_envelope_carries_values_and_links_and_round_trips() {
    let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let outcome = Outcome::new(TestCode::Failed)
        .stage(TestLocation::ReadIndex)
        .path(r"C:\state\index.json")
        .caused_by(Outcome::from_io_error(&error, IoOp::Open).path(r"C:\state\index.json"));
    let text = serde_json::to_string(&outcome.envelope(Detail::Full)).expect("serialize");
    let value: JsonValue = serde_json::from_str(&text).expect("parse");
    assert_eq!(value["schema_version"], OUTCOME_SCHEMA_VERSION);
    assert_eq!(value["code"], "test_operation_failed");
    assert_eq!(value["category"], "error");
    assert_eq!(value["values"]["stage"], "test_read_index");
    assert_eq!(value["causes"][0]["relation"], "caused_by");
    assert_eq!(value["causes"][0]["code"], "foreign_os_error");
    assert_eq!(value["causes"][0]["values"]["raw_source"], "os");
    assert_eq!(value["causes"][0]["values"]["io_kind"], "permission_denied");
    assert_eq!(value["causes"][0]["values"]["io_op"], "open");
    assert_eq!(value["causes"][0]["values"]["raw_text"], "access denied");
    assert!(value.get("detail").is_none());

    let received: OutcomeEnvelope = serde_json::from_str(&text).expect("read back");
    assert_eq!(
        serde_json::to_string(&received).expect("serialize again"),
        text
    );
}

#[test]
fn codes_mode_keeps_only_registered_names() {
    let error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let outcome = Outcome::new(TestCode::Failed)
        .operation(TestLocation::ReadIndex)
        .path(r"C:\state\index.json")
        .caused_by(Outcome::from_io_error(&error, IoOp::Read));
    let value = serde_json::to_value(outcome.envelope(Detail::Codes)).expect("serialize");
    assert_eq!(value["detail"], "codes");
    assert_eq!(
        value["values"],
        serde_json::json!({"operation": "test_read_index"})
    );
    assert_eq!(
        value["causes"][0]["values"],
        serde_json::json!({"raw_source": "os", "io_kind": "not_found", "io_op": "read"})
    );
}

#[test]
fn links_are_cut_to_thirty_two_and_to_depth_eight() {
    let mut wide = Outcome::new(TestCode::Failed);
    for _ in 0..40 {
        wide = wide.link(CauseRelation::Related, Outcome::new(TestCode::Done));
    }
    let value = serde_json::to_value(wide.envelope(Detail::Full)).expect("serialize");
    assert_eq!(
        value["causes"].as_array().map(Vec::len),
        Some(CAUSES_MAX_LINKS)
    );
    assert_eq!(value["causes_total"], 40);

    let mut deep = Outcome::new(TestCode::Done);
    for _ in 0..12 {
        deep = Outcome::new(TestCode::Failed).caused_by(deep);
    }
    let value = serde_json::to_value(deep.envelope(Detail::Full)).expect("serialize");
    let mut depth = 0;
    let mut node = &value;
    while let Some(next) = node["causes"].get(0) {
        depth += 1;
        node = next;
    }
    assert_eq!(depth, CAUSES_MAX_DEPTH);
    assert_eq!(value["causes_total"], 12);
}

#[test]
fn raw_text_is_cut_at_its_bound_and_the_full_text_goes_to_the_sink() {
    recording_sink();
    let long = "x".repeat(RAW_TEXT_MAX_BYTES + 10);
    let outcome = Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Other)
        .raw_text(long.as_bytes());
    assert_eq!(
        outcome
            .value("raw_text")
            .and_then(JsonValue::as_str)
            .map(str::len),
        Some(RAW_TEXT_MAX_BYTES)
    );
    assert_eq!(
        outcome.value("raw_text_truncated"),
        Some(&JsonValue::Bool(true))
    );
    let expected = format!("foreign_other_error {}", RAW_TEXT_MAX_BYTES + 10);
    assert!(
        SINK_TEXTS
            .lock()
            .expect("lock the test sink")
            .iter()
            .any(|entry| *entry == expected)
    );
}

#[test]
fn a_received_envelope_with_an_unknown_code_stays_readable() {
    let text = r#"{"schema_version":"actingcommand.outcome.v1","code":"newer_code","category":"warning","values":{"later_key":1},"later_member":true}"#;
    let envelope: OutcomeEnvelope = serde_json::from_str(text).expect("read");
    assert_eq!(envelope.outcome().code().as_str(), "newer_code");
    assert_eq!(envelope.outcome().category(), Category::Warning);
    let relayed =
        Outcome::new(TestCode::Failed).link(CauseRelation::Related, envelope.into_outcome());
    assert_eq!(
        relayed.to_string(),
        "test_operation_failed [related: newer_code later_key=1]"
    );
}

#[test]
fn the_embedded_catalog_is_the_merged_file() {
    let value: JsonValue = serde_json::from_str(catalog()).expect("parse the catalog");
    assert_eq!(value["schema_version"], CATALOG_SCHEMA_VERSION);
    assert_eq!(value["codes"]["foreign_os_error"]["category"], "error");
    assert_eq!(value["codes"]["panic_caught"]["category"], "warning");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "fatal link")]
fn a_fatal_link_needs_a_fatal_top() {
    let _ = Outcome::new(TestCode::Failed)
        .caused_by(Outcome::new(TestCode::Unusable))
        .envelope(Detail::Full);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "required key raw_text is missing")]
fn a_missing_required_key_fails_in_debug_builds() {
    let _ = Outcome::new(ContractCode::ForeignOsError)
        .raw_source(RawSource::Os)
        .envelope(Detail::Full);
}
