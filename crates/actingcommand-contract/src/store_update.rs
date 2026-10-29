// SPDX-License-Identifier: AGPL-3.0-only

//! STORE-FG-1 (design `rd-1029/design-store-fg.md` §2, §3; since 0.12.29): a package's
//! `control.json` may declare `store_update`, the steps it runs while the store application it
//! names is in front. The shape and its own rules live here, so the source parser and kernel
//! admission read one grammar; the cross-checks against the task and its pages are kernel
//! admission's.
//!
//! ```json
//! "store_update": {
//!   "store_application_id": "test.store.alpha",
//!   "store_operations": [{"operation": "tap_update", "proof": ["listing_title"]}]
//! }
//! ```
//!
//! The Runtime compares package ids for equality only and names no store.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most store operations one package declares.
pub const MAX_STORE_OPERATIONS: usize = 8;
/// The most proof targets one store operation names.
pub const MAX_STORE_PROOF_TARGETS: usize = 4;
/// The longest Android package id a store application id may be, in bytes.
pub const MAX_STORE_APPLICATION_ID_BYTES: usize = 256;
/// The longest operation id or proof target id, in bytes.
const MAX_STORE_REFERENCE_BYTES: usize = 256;

/// `control.json` `store_update`: the store application the package's store steps run in, and
/// those steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreUpdateDeclaration {
    /// The Android package id of the store application, which must be in front when a store
    /// step taps.
    pub store_application_id: String,
    /// The operations of the package's task that tap while the store is in front, each with the
    /// targets that prove the screen is the package's own listing.
    pub store_operations: Vec<StoreOperationDeclaration>,
}

/// One store step: an operation of the package's task and its listing proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreOperationDeclaration {
    /// The id of the operation (`operation` in `control.json`).
    #[serde(rename = "operation")]
    pub operation_id: String,
    /// 1 to [`MAX_STORE_PROOF_TARGETS`] OCR targets that stand directly in the `required` list of
    /// the operation's `from` page.
    pub proof: Vec<String>,
}

impl StoreUpdateDeclaration {
    /// The store application a tap of the operation `operation_id` goes to, when it is one of the
    /// declared store operations.
    pub fn store_for(&self, operation_id: &str) -> Option<&str> {
        self.store_operations
            .iter()
            .any(|declared| declared.operation_id == operation_id)
            .then_some(self.store_application_id.as_str())
    }
}

/// A refused `store_update`: the JSON pointer, in `control.json`, of the member that breaks a
/// rule. The refusal's reason is always `store_update_invalid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreUpdateViolation {
    pub pointer: String,
}

impl StoreUpdateViolation {
    fn at(pointer: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
        }
    }
}

/// An Android package name: at least two dot-separated segments, each a letter followed by
/// letters, digits or `_`, at most [`MAX_STORE_APPLICATION_ID_BYTES`] bytes in all. STORE-FG-2's
/// configuration list takes the same grammar.
pub fn android_package_id_valid(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_STORE_APPLICATION_ID_BYTES {
        return false;
    }
    let mut segments = 0_usize;
    for segment in id.split('.') {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_alphabetic()
            || !chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

/// A reference to an operation or a target: not blank, at most 256 bytes, no control character.
fn reference_valid(reference: &str) -> bool {
    !reference.trim().is_empty()
        && reference.len() <= MAX_STORE_REFERENCE_BYTES
        && !reference.chars().any(char::is_control)
}

/// The rules of the member itself (design §3, `reason=store_update_invalid`): the store
/// application id is an Android package name; 1 to [`MAX_STORE_OPERATIONS`] store operations,
/// their operation ids valid and unique; each with 1 to [`MAX_STORE_PROOF_TARGETS`] proof
/// targets, valid and unique within the operation. The first broken rule is refused at its
/// member.
pub fn validate_store_update(
    declaration: &StoreUpdateDeclaration,
) -> Result<(), StoreUpdateViolation> {
    const ROOT: &str = "/store_update";
    if !android_package_id_valid(&declaration.store_application_id) {
        return Err(StoreUpdateViolation::at(format!(
            "{ROOT}/store_application_id"
        )));
    }
    let operations = &declaration.store_operations;
    if operations.is_empty() || operations.len() > MAX_STORE_OPERATIONS {
        return Err(StoreUpdateViolation::at(format!("{ROOT}/store_operations")));
    }
    let mut seen = BTreeSet::new();
    for (index, operation) in operations.iter().enumerate() {
        let pointer = format!("{ROOT}/store_operations/{index}");
        if !reference_valid(&operation.operation_id)
            || !seen.insert(operation.operation_id.as_str())
        {
            return Err(StoreUpdateViolation::at(format!("{pointer}/operation")));
        }
        if operation.proof.is_empty() || operation.proof.len() > MAX_STORE_PROOF_TARGETS {
            return Err(StoreUpdateViolation::at(format!("{pointer}/proof")));
        }
        let mut proof = BTreeSet::new();
        for (target_index, target) in operation.proof.iter().enumerate() {
            if !reference_valid(target) || !proof.insert(target.as_str()) {
                return Err(StoreUpdateViolation::at(format!(
                    "{pointer}/proof/{target_index}"
                )));
            }
        }
    }
    Ok(())
}
