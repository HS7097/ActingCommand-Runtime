// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{EnvResolved, LabError, LabResult};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Validated environment facts supplied to the deterministic resource compiler.
#[derive(Debug, Clone, Default)]
pub struct AuthoringEnvironmentSnapshot {
    values: BTreeMap<String, EnvResolved>,
}

impl AuthoringEnvironmentSnapshot {
    pub fn from_resolved(values: impl IntoIterator<Item = EnvResolved>) -> LabResult<Self> {
        let mut snapshot = Self::default();
        for value in values {
            validate_fact(&value)?;
            if snapshot.values.insert(value.key.clone(), value).is_some() {
                return Err(LabError::usage(
                    "resolved environment snapshot contains a duplicate key",
                ));
            }
        }
        Ok(snapshot)
    }

    pub(crate) fn apply(&self, value: &mut Value) -> LabResult<()> {
        match value {
            Value::String(text) => *text = self.resolve_text(text)?,
            Value::Array(values) => {
                for value in values {
                    self.apply(value)?;
                }
            }
            Value::Object(object) => {
                for value in object.values_mut() {
                    self.apply(value)?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
        Ok(())
    }

    fn resolve_text(&self, input: &str) -> LabResult<String> {
        let mut output = String::new();
        let mut offset = 0usize;
        while let Some(start_rel) = input[offset..].find("{env:") {
            let start = offset + start_rel;
            output.push_str(&input[offset..start]);
            let key_start = start + "{env:".len();
            let end_rel = input[key_start..]
                .find('}')
                .ok_or_else(|| LabError::usage("malformed env pointer: missing closing brace"))?;
            let end = key_start + end_rel;
            let key = &input[key_start..end];
            let value = self.values.get(key).ok_or_else(|| {
                LabError::usage(format!(
                    "resolved environment snapshot is missing key '{key}'"
                ))
            })?;
            output.push_str(&value.value);
            offset = end + 1;
        }
        output.push_str(&input[offset..]);
        Ok(output)
    }
}

pub(crate) fn required_environment_keys(value: &Value) -> LabResult<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    collect_required_keys(value, &mut keys)?;
    Ok(keys)
}

fn collect_required_keys(value: &Value, keys: &mut BTreeSet<String>) -> LabResult<()> {
    match value {
        Value::String(text) => collect_text_keys(text, keys)?,
        Value::Array(values) => {
            for value in values {
                collect_required_keys(value, keys)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_required_keys(value, keys)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn collect_text_keys(text: &str, keys: &mut BTreeSet<String>) -> LabResult<()> {
    let mut offset = 0usize;
    while let Some(start_rel) = text[offset..].find("{env:") {
        let key_start = offset + start_rel + "{env:".len();
        let end_rel = text[key_start..]
            .find('}')
            .ok_or_else(|| LabError::usage("malformed env pointer: missing closing brace"))?;
        let end = key_start + end_rel;
        let key = &text[key_start..end];
        if key.trim().is_empty() {
            return Err(LabError::usage("env pointer key must not be empty"));
        }
        keys.insert(key.to_string());
        offset = end + 1;
    }
    Ok(())
}

fn validate_fact(fact: &EnvResolved) -> LabResult<()> {
    if fact.key.trim().is_empty() {
        return Err(LabError::usage(
            "resolved environment fact key must not be empty",
        ));
    }
    if fact.value.is_empty()
        || fact.value == "."
        || fact.value.contains('/')
        || fact.value.contains('\\')
        || fact.value.contains(':')
        || fact.value.contains("..")
        || Path::new(&fact.value).is_absolute()
    {
        return Err(LabError::usage(format!(
            "resolved environment key '{}' has an unsafe value",
            fact.key
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fact(key: &str, value: &str) -> EnvResolved {
        EnvResolved {
            key: key.to_string(),
            value: value.to_string(),
            confidence: 1.0,
            source: "sealed".to_string(),
            detector_id: "detector".to_string(),
            source_result: "detector@1".to_string(),
        }
    }

    #[test]
    fn snapshot_applies_nested_markers_and_reports_required_keys() {
        let snapshot = AuthoringEnvironmentSnapshot::from_resolved([
            fact("server", "region-b"),
            fact("theme", "default"),
        ])
        .expect("snapshot");
        let mut value = json!({"path":"{env:server}/x", "nested":["{env:theme}"]});

        assert_eq!(
            required_environment_keys(&value).expect("keys"),
            BTreeSet::from(["server".to_string(), "theme".to_string()])
        );
        snapshot.apply(&mut value).expect("apply");
        assert_eq!(value, json!({"path":"region-b/x", "nested":["default"]}));
    }

    #[test]
    fn snapshot_rejects_missing_duplicate_unsafe_and_malformed_values() {
        let duplicate = AuthoringEnvironmentSnapshot::from_resolved([
            fact("server", "region-b"),
            fact("server", "region-a"),
        ])
        .expect_err("duplicate");
        assert_eq!(duplicate.code, "validation_failed");

        let unsafe_value =
            AuthoringEnvironmentSnapshot::from_resolved([fact("server", "../region-b")])
                .expect_err("unsafe");
        assert_eq!(unsafe_value.code, "validation_failed");

        let snapshot = AuthoringEnvironmentSnapshot::default();
        let mut missing = json!("{env:server}");
        assert!(snapshot.apply(&mut missing).is_err());
        assert!(required_environment_keys(&json!("{env:server")).is_err());
    }
}
