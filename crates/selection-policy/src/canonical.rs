// SPDX-License-Identifier: AGPL-3.0-only

//! Canonical JSON form and document identity for selection-policy inputs.
//!
//! The form follows the JCS shape already used by the scheduling catalog: object keys sorted
//! by their UTF-16 code units, no insignificant whitespace, arrays left in order. On top of
//! that it refuses anything that would make a hash ambiguous: floating point numbers,
//! integers outside the ECMAScript safe range, duplicate object keys, and non-string keys.
//!
//! Workflow #357 DB-1 (M-357a v1.1 §2.2): the reader lives in the contract crate
//! (`actingcommand_contract::canonical`), which the standard-pack table reader shares; this
//! module re-exports it with the selection-policy error type and this crate's document bound,
//! bytes and behaviour unchanged.

use serde::Serialize;
use sha2::{Digest, Sha256};

use actingcommand_contract::canonical::{
    self as contract, CanonicalJsonError, CanonicalJsonErrorKind,
};

use crate::schema::{SelectionError, SelectionErrorCode};

pub use actingcommand_contract::canonical::CanonicalValue;

/// Upper bound on one decoded document, matching the planning-document envelope allowance.
pub const MAX_DOCUMENT_BYTES: usize = 512 * 1024;

/// The selection-policy error of a contract refusal: the same kind, the same text.
fn selection_error(error: CanonicalJsonError) -> SelectionError {
    let code = match error.kind {
        CanonicalJsonErrorKind::InvalidJson => SelectionErrorCode::InvalidJson,
        CanonicalJsonErrorKind::DuplicateKey => SelectionErrorCode::DuplicateKey,
        CanonicalJsonErrorKind::FloatRejected => SelectionErrorCode::FloatRejected,
        CanonicalJsonErrorKind::IntegerOutOfRange => SelectionErrorCode::IntegerOutOfRange,
    };
    SelectionError::new(code, error.detail)
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
    contract::parse_canonical_json(bytes).map_err(selection_error)
}

/// Renders any serializable value into the canonical byte form.
pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, SelectionError> {
    let value = serde_json::to_value(value).map_err(|error| {
        SelectionError::new(
            SelectionErrorCode::InvalidJson,
            format!("serialize: {error}"),
        )
    })?;
    let value = contract::canonical_value_of(&value).map_err(selection_error)?;
    let mut output = Vec::new();
    value.write(&mut output).map_err(selection_error)?;
    Ok(output)
}

/// Returns the `sha256:<hex>` identity of a value's canonical byte form.
pub fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, SelectionError> {
    let digest = Sha256::digest(canonical_bytes(value)?);
    Ok(format!("sha256:{digest:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_sorts_object_keys_and_keeps_array_order() {
        let value = parse_canonical_json(br#"{"z":[2,1],"a":{"b":true,"a":null}}"#)
            .expect("canonical parse");
        let mut output = Vec::new();
        value.write(&mut output).expect("canonical write");
        assert_eq!(output, br#"{"a":{"a":null,"b":true},"z":[2,1]}"#);
    }

    #[test]
    fn canonical_parse_rejects_every_floating_point_spelling() {
        for source in [
            br#"{"value":1.5}"#.as_slice(),
            br#"{"value":1.0}"#.as_slice(),
            br#"{"value":1e3}"#.as_slice(),
        ] {
            let error = parse_canonical_json(source).expect_err("floating point must fail");
            assert_eq!(error.code(), SelectionErrorCode::FloatRejected, "{error}");
        }
    }

    #[test]
    fn canonical_parse_rejects_duplicate_and_non_string_keys() {
        let duplicate = parse_canonical_json(br#"{"a":1,"a":2}"#).expect_err("duplicate key");
        assert_eq!(duplicate.code(), SelectionErrorCode::DuplicateKey);
        let non_string = parse_canonical_json(br#"{1:2}"#).expect_err("non-string key");
        assert_eq!(non_string.code(), SelectionErrorCode::InvalidJson);
    }

    #[test]
    fn canonical_parse_enforces_the_safe_integer_boundaries() {
        for accepted in [
            br#"-9007199254740991"#.as_slice(),
            br#"9007199254740991"#.as_slice(),
        ] {
            parse_canonical_json(accepted).expect("safe integer");
        }
        for rejected in [
            br#"-9007199254740992"#.as_slice(),
            br#"9007199254740992"#.as_slice(),
        ] {
            let error = parse_canonical_json(rejected).expect_err("unsafe integer");
            assert_eq!(error.code(), SelectionErrorCode::IntegerOutOfRange);
        }
    }

    #[test]
    fn canonical_rendering_rejects_a_serialized_float() {
        #[derive(serde::Serialize)]
        struct Holder {
            value: f64,
        }
        let error = canonical_bytes(&Holder { value: 0.5 }).expect_err("floating point must fail");
        assert_eq!(error.code(), SelectionErrorCode::FloatRejected);
    }

    #[test]
    fn canonical_identity_is_stable_across_key_order() {
        let left = canonical_sha256(&serde_json::json!({"a": 1, "z": 2})).expect("identity");
        let right = canonical_sha256(&serde_json::json!({"z": 2, "a": 1})).expect("identity");
        assert_eq!(left, right);
        assert!(left.starts_with("sha256:"));
    }

    #[test]
    fn canonical_parse_rejects_an_oversized_document() {
        let oversized = vec![b' '; MAX_DOCUMENT_BYTES + 1];
        let error = parse_canonical_json(&oversized).expect_err("oversized document");
        assert_eq!(error.code(), SelectionErrorCode::DocumentTooLarge);
    }
}
