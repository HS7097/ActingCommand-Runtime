// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1a (model-398 v1.1 §12 row T6, §10.2, §3.5 rules 4 and 7): the key
//! `adb_serial` is retired for `device_label`, `foreign_adb_error` may carry `device_label` and
//! `raw_text_redacted`, and the key table agrees with the catalog header. With row T1's stored
//! form: the adb command observation round-trips its `raw_text_redacted` flag.

use actingcommand_contract::outcome::{KeyKind, Record, key_kind};
use actingcommand_contract::{AdbCommandEvidence, AdbTransportState, LifecycleNativeDetail};
use serde_json::Value;
use std::path::PathBuf;

fn catalog_file(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/outcome-codes")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("catalog JSON")
}

#[test]
fn rd1a_t6_device_label_replaces_adb_serial_in_keys_and_foreign_adb_error() {
    // A label carries the instance alias as configured (`handset:<alias>`, upper case and
    // CJK allowed) or an IPv6 `HOST:PORT`, so the key takes the alias key's type, `name`
    // (train 3 review fix), and such labels pass the key check.
    assert_eq!(key_kind("device_label"), Some(KeyKind::Name));
    assert_eq!(key_kind("device_label"), key_kind("alias"));
    for label in ["handset:Bench-\u{6d4b}\u{8bd5}", "[2001:db8::42]:16416"] {
        let record = Record::new().device_label(label);
        assert_eq!(
            record.values().get("device_label").and_then(Value::as_str),
            Some(label)
        );
    }
    assert_eq!(key_kind("raw_text_redacted"), Some(KeyKind::Boolean));
    assert_eq!(key_kind("adb_serial"), None);

    let header = catalog_file("catalog.json");
    assert_eq!(
        header
            .pointer("/keys/device_label/type")
            .and_then(Value::as_str),
        Some("name")
    );
    assert_eq!(
        header
            .pointer("/keys/raw_text_redacted/type")
            .and_then(Value::as_str),
        Some("boolean")
    );
    assert!(header.pointer("/keys/adb_serial").is_none());

    let fragment = catalog_file("contract.json");
    let values = fragment
        .pointer("/codes/foreign_adb_error/values")
        .and_then(Value::as_object)
        .expect("the adb fallback code's values");
    assert_eq!(
        values.get("device_label").and_then(Value::as_str),
        Some("optional")
    );
    assert_eq!(
        values.get("raw_text_redacted").and_then(Value::as_str),
        Some("optional")
    );
    assert!(!values.contains_key("adb_serial"));
}

#[test]
fn rd1a_t1_stored_adb_command_evidence_round_trips_raw_text_redacted() {
    let redacted = AdbCommandEvidence {
        succeeded: false,
        exit_code: Some(1),
        stdout: None,
        stderr: Some(LifecycleNativeDetail::new(
            "error: device '[device:handset:neutral.handset]' not found",
            false,
        )),
        stdout_lossy_decode: false,
        stderr_lossy_decode: false,
        state: AdbTransportState::Unknown,
        raw_text_redacted: true,
        outcome: None,
    };
    let encoded = serde_json::to_value(&redacted).expect("encode");
    assert_eq!(encoded.get("raw_text_redacted"), Some(&Value::Bool(true)));
    assert_eq!(
        serde_json::from_value::<AdbCommandEvidence>(encoded).expect("decode"),
        redacted
    );

    // Nothing replaced: the member is absent, so an emulator's record keeps its old shape.
    let plain = AdbCommandEvidence {
        raw_text_redacted: false,
        ..redacted
    };
    let encoded = serde_json::to_value(&plain).expect("encode");
    assert!(encoded.get("raw_text_redacted").is_none());
    assert_eq!(
        serde_json::from_value::<AdbCommandEvidence>(encoded).expect("decode"),
        plain
    );
}
