// SPDX-License-Identifier: AGPL-3.0-only

//! Canonical JSON form and document identity for selection-policy inputs.
//!
//! The form follows the JCS shape already used by the scheduling catalog: object keys sorted
//! by their UTF-16 code units, no insignificant whitespace, arrays left in order. On top of
//! that it refuses anything that would make a hash ambiguous: floating point numbers,
//! integers outside the ECMAScript safe range, duplicate object keys, and non-string keys.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::schema::{SelectionError, SelectionErrorCode};

/// Upper bound on one decoded document, matching the planning-document envelope allowance.
pub const MAX_DOCUMENT_BYTES: usize = 512 * 1024;

const SAFE_INTEGER_MIN: i64 = -9_007_199_254_740_991;
const SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

/// The closed JSON value model that survives canonicalization without loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Text(String),
    Array(Vec<CanonicalValue>),
    Object(BTreeMap<String, CanonicalValue>),
}

impl CanonicalValue {
    fn write(&self, output: &mut Vec<u8>) -> Result<(), SelectionError> {
        match self {
            Self::Null => output.extend_from_slice(b"null"),
            Self::Boolean(true) => output.extend_from_slice(b"true"),
            Self::Boolean(false) => output.extend_from_slice(b"false"),
            Self::Integer(value) => output.extend_from_slice(value.to_string().as_bytes()),
            Self::Text(text) => write_string(text, output)?,
            Self::Array(items) => {
                output.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    item.write(output)?;
                }
                output.push(b']');
            }
            Self::Object(entries) => {
                let mut keys: Vec<&str> = entries.keys().map(String::as_str).collect();
                keys.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
                output.push(b'{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    write_string(key, output)?;
                    output.push(b':');
                    entries[key].write(output)?;
                }
                output.push(b'}');
            }
        }
        Ok(())
    }
}

fn write_string(text: &str, output: &mut Vec<u8>) -> Result<(), SelectionError> {
    let encoded = serde_json::to_string(text).map_err(|error| {
        SelectionError::new(SelectionErrorCode::InvalidJson, format!("string: {error}"))
    })?;
    output.extend_from_slice(encoded.as_bytes());
    Ok(())
}

/// Parses one document, rejecting the shapes that make a canonical hash ambiguous.
pub fn parse_canonical_json(bytes: &[u8]) -> Result<CanonicalValue, SelectionError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SelectionError::new(
            SelectionErrorCode::DocumentTooLarge,
            format!(
                "{} bytes exceed the {MAX_DOCUMENT_BYTES} byte limit",
                bytes.len()
            ),
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = CanonicalValue::deserialize(&mut deserializer).map_err(classify)?;
    deserializer.end().map_err(classify)?;
    Ok(value)
}

/// Renders any serializable value into the canonical byte form.
pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, SelectionError> {
    let value = serde_json::to_value(value).map_err(|error| {
        SelectionError::new(
            SelectionErrorCode::InvalidJson,
            format!("serialize: {error}"),
        )
    })?;
    let value = from_serde_value(&value)?;
    let mut output = Vec::new();
    value.write(&mut output)?;
    Ok(output)
}

/// Returns the `sha256:<hex>` identity of a value's canonical byte form.
pub fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, SelectionError> {
    let digest = Sha256::digest(canonical_bytes(value)?);
    Ok(format!("sha256:{digest:x}"))
}

fn from_serde_value(value: &Value) -> Result<CanonicalValue, SelectionError> {
    Ok(match value {
        Value::Null => CanonicalValue::Null,
        Value::Bool(value) => CanonicalValue::Boolean(*value),
        Value::Number(number) => CanonicalValue::Integer(safe_integer(number)?),
        Value::String(text) => CanonicalValue::Text(text.clone()),
        Value::Array(items) => CanonicalValue::Array(
            items
                .iter()
                .map(from_serde_value)
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => {
            let mut object = BTreeMap::new();
            for (key, entry) in entries {
                if object
                    .insert(key.clone(), from_serde_value(entry)?)
                    .is_some()
                {
                    return Err(SelectionError::new(
                        SelectionErrorCode::DuplicateKey,
                        format!("duplicate object key `{key}`"),
                    ));
                }
            }
            CanonicalValue::Object(object)
        }
    })
}

fn safe_integer(number: &serde_json::Number) -> Result<i64, SelectionError> {
    let value = number.as_i64().ok_or_else(|| {
        if number.as_u64().is_some() {
            SelectionError::new(
                SelectionErrorCode::IntegerOutOfRange,
                format!("`{number}` is outside the safe integer range"),
            )
        } else {
            SelectionError::new(
                SelectionErrorCode::FloatRejected,
                format!("`{number}` is not an integer"),
            )
        }
    })?;
    if !(SAFE_INTEGER_MIN..=SAFE_INTEGER_MAX).contains(&value) {
        return Err(SelectionError::new(
            SelectionErrorCode::IntegerOutOfRange,
            format!("`{value}` is outside the safe integer range"),
        ));
    }
    Ok(value)
}

fn classify(error: serde_json::Error) -> SelectionError {
    let message = error.to_string();
    let code = if message.contains("duplicate object key") {
        SelectionErrorCode::DuplicateKey
    } else if message.contains("floating point") {
        SelectionErrorCode::FloatRejected
    } else if message.contains("safe integer range") {
        SelectionErrorCode::IntegerOutOfRange
    } else {
        SelectionErrorCode::InvalidJson
    };
    SelectionError::new(code, message)
}

impl<'de> Deserialize<'de> for CanonicalValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CanonicalValueVisitor)
    }
}

struct CanonicalValueVisitor;

impl<'de> Visitor<'de> for CanonicalValueVisitor {
    type Value = CanonicalValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("canonical JSON without floating point or duplicate keys")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(CanonicalValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(CanonicalValue::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        Ok(CanonicalValue::Boolean(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        if !(SAFE_INTEGER_MIN..=SAFE_INTEGER_MAX).contains(&value) {
            return Err(E::custom(format!(
                "`{value}` is outside the safe integer range"
            )));
        }
        Ok(CanonicalValue::Integer(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        if value > SAFE_INTEGER_MAX as u64 {
            return Err(E::custom(format!(
                "`{value}` is outside the safe integer range"
            )));
        }
        Ok(CanonicalValue::Integer(value as i64))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Err(E::custom(format!("`{value}` is a floating point number")))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(CanonicalValue::Text(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(CanonicalValue::Text(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = access.next_element()? {
            items.push(item);
        }
        Ok(CanonicalValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut entries: BTreeMap<String, CanonicalValue> = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value()?;
            if entries.insert(key.clone(), value).is_some() {
                return Err(de::Error::custom(format!("duplicate object key `{key}`")));
            }
        }
        Ok(CanonicalValue::Object(entries))
    }
}
