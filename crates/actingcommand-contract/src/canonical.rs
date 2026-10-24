// SPDX-License-Identifier: AGPL-3.0-only

//! Canonical JSON: the one reader of documents whose bytes must not be ambiguous (Workflow
//! #357 DB-1, M-357a v1.1 §2.2: moved here from `crates/selection-policy` so that the contract's
//! standard-pack table reader can use it; selection-policy re-exports it with its own error type,
//! bytes and behaviour unchanged).
//!
//! The form follows the JCS shape already used by the scheduling catalog: object keys sorted
//! by their UTF-16 code units, no insignificant whitespace, arrays left in order. On top of
//! that it refuses anything that would make a hash ambiguous: floating point numbers,
//! integers outside the ECMAScript safe range, duplicate object keys, and non-string keys.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// The smallest integer canonical JSON holds (the ECMAScript safe range).
pub const CANONICAL_SAFE_INTEGER_MIN: i64 = -9_007_199_254_740_991;
/// The largest integer canonical JSON holds (the ECMAScript safe range).
pub const CANONICAL_SAFE_INTEGER_MAX: i64 = 9_007_199_254_740_991;

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

/// What kind of input canonical JSON refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalJsonErrorKind {
    /// Not JSON, a non-string object key, or a value that does not serialize.
    InvalidJson,
    /// An object key appears twice.
    DuplicateKey,
    /// A number that is not an integer.
    FloatRejected,
    /// An integer outside the safe range.
    IntegerOutOfRange,
}

/// A refusal of canonical JSON: its kind and developer text naming the offending input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalJsonError {
    pub kind: CanonicalJsonErrorKind,
    pub detail: String,
}

impl CanonicalJsonError {
    fn new(kind: CanonicalJsonErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for CanonicalJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for CanonicalJsonError {}

impl CanonicalValue {
    /// Appends the value's canonical byte form to `output`.
    pub fn write(&self, output: &mut Vec<u8>) -> Result<(), CanonicalJsonError> {
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

fn write_string(text: &str, output: &mut Vec<u8>) -> Result<(), CanonicalJsonError> {
    let encoded = serde_json::to_string(text).map_err(|error| {
        CanonicalJsonError::new(
            CanonicalJsonErrorKind::InvalidJson,
            format!("string: {error}"),
        )
    })?;
    output.extend_from_slice(encoded.as_bytes());
    Ok(())
}

/// Parses one document, rejecting the shapes that make a canonical hash ambiguous. The caller
/// bounds the document's size.
pub fn parse_canonical_json(bytes: &[u8]) -> Result<CanonicalValue, CanonicalJsonError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = CanonicalValue::deserialize(&mut deserializer).map_err(classify)?;
    deserializer.end().map_err(classify)?;
    Ok(value)
}

/// The canonical value of a decoded JSON value, refusing what canonical JSON cannot hold.
pub fn canonical_value_of(value: &Value) -> Result<CanonicalValue, CanonicalJsonError> {
    Ok(match value {
        Value::Null => CanonicalValue::Null,
        Value::Bool(value) => CanonicalValue::Boolean(*value),
        Value::Number(number) => CanonicalValue::Integer(safe_integer(number)?),
        Value::String(text) => CanonicalValue::Text(text.clone()),
        Value::Array(items) => CanonicalValue::Array(
            items
                .iter()
                .map(canonical_value_of)
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => {
            let mut object = BTreeMap::new();
            for (key, entry) in entries {
                if object
                    .insert(key.clone(), canonical_value_of(entry)?)
                    .is_some()
                {
                    return Err(CanonicalJsonError::new(
                        CanonicalJsonErrorKind::DuplicateKey,
                        format!("duplicate object key `{key}`"),
                    ));
                }
            }
            CanonicalValue::Object(object)
        }
    })
}

fn safe_integer(number: &serde_json::Number) -> Result<i64, CanonicalJsonError> {
    let value = number.as_i64().ok_or_else(|| {
        if number.as_u64().is_some() {
            CanonicalJsonError::new(
                CanonicalJsonErrorKind::IntegerOutOfRange,
                format!("`{number}` is outside the safe integer range"),
            )
        } else {
            CanonicalJsonError::new(
                CanonicalJsonErrorKind::FloatRejected,
                format!("`{number}` is not an integer"),
            )
        }
    })?;
    if !(CANONICAL_SAFE_INTEGER_MIN..=CANONICAL_SAFE_INTEGER_MAX).contains(&value) {
        return Err(CanonicalJsonError::new(
            CanonicalJsonErrorKind::IntegerOutOfRange,
            format!("`{value}` is outside the safe integer range"),
        ));
    }
    Ok(value)
}

fn classify(error: serde_json::Error) -> CanonicalJsonError {
    let message = error.to_string();
    let kind = if message.contains("duplicate object key") {
        CanonicalJsonErrorKind::DuplicateKey
    } else if message.contains("floating point") {
        CanonicalJsonErrorKind::FloatRejected
    } else if message.contains("safe integer range") {
        CanonicalJsonErrorKind::IntegerOutOfRange
    } else {
        CanonicalJsonErrorKind::InvalidJson
    };
    CanonicalJsonError::new(kind, message)
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
        if !(CANONICAL_SAFE_INTEGER_MIN..=CANONICAL_SAFE_INTEGER_MAX).contains(&value) {
            return Err(E::custom(format!(
                "`{value}` is outside the safe integer range"
            )));
        }
        Ok(CanonicalValue::Integer(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        if value > CANONICAL_SAFE_INTEGER_MAX as u64 {
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
