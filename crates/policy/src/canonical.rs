// SPDX-License-Identifier: AGPL-3.0-only

use std::cmp::Ordering;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::CatalogBundle;

pub(crate) fn catalog_hash(bundle: &CatalogBundle) -> Result<String, String> {
    let envelope = serde_json::json!({
        "activity": &bundle.activity,
        "pools": &bundle.pools,
        "tasks": &bundle.tasks,
        "timeline": &bundle.timeline,
    });
    let canonical = canonical_json(&envelope)?;
    let digest = Sha256::digest(canonical);
    Ok(format!("sha256:{digest:x}"))
}

pub(crate) fn canonical_serialized<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    canonical_json(&value)
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    write_value(value, &mut output)?;
    Ok(output)
}

fn write_value(value: &Value, output: &mut Vec<u8>) -> Result<(), String> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            output.extend_from_slice(number.to_string().as_bytes());
        }
        Value::Number(_) => {
            return Err("scheduling canonicalization accepts integers only".to_owned());
        }
        Value::String(text) => {
            let encoded = serde_json::to_string(text).map_err(|error| error.to_string())?;
            output.extend_from_slice(encoded.as_bytes());
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, item) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_value(item, output)?;
            }
            output.push(b']');
        }
        Value::Object(entries) => {
            let mut keys: Vec<&str> = entries.keys().map(String::as_str).collect();
            keys.sort_by(|left, right| compare_utf16(left, right));
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                let encoded = serde_json::to_string(key).map_err(|error| error.to_string())?;
                output.extend_from_slice(encoded.as_bytes());
                output.push(b':');
                write_value(&entries[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn compare_utf16(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_object_keys_and_preserves_arrays() {
        let value = serde_json::json!({"z": [2, 1], "a": {"b": true, "a": null}});
        let encoded = canonical_json(&value).expect("canonical JSON");
        assert_eq!(encoded, br#"{"a":{"a":null,"b":true},"z":[2,1]}"#);
    }

    #[test]
    fn canonical_json_rejects_floating_point_values() {
        let error = canonical_json(&serde_json::json!({"value": 1.5}))
            .expect_err("floating point input must fail");
        assert!(error.contains("integers only"));
    }
}
