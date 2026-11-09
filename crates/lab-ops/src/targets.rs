// SPDX-License-Identifier: AGPL-3.0-only

//! The resource target operations (Workflow #434 LAB-1b; register §1.287), moved from MCP's
//! `ac_resources_list`, `ac_targets_get` and `ac_targets_set` and from actingctl's
//! `agent-apply-resource-targets`. Whether a policy is accepted is the Runtime's answer, passed
//! on unchanged:
//!
//! - `targets` (Workflow #338 R5): one `ResourceTargetView` of an instance, answered as what it
//!   can target ([`resources_answer`]) or as the policy it holds ([`active_answer`]);
//! - `targets set`: one policy document through the formal entry, `ApplyResourceTargets`
//!   (Workflow #308 RT-S1a): read from a file ([`read_document`]), or built from MCP's targets
//!   for the instance's `policy_instance` ([`document`]).

use crate::codes::{ActingctlCode, ResourceTargetsRejectionReason};
use crate::{InputProblem, read_bounded};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    MAX_RESOURCE_TARGETS_DOCUMENT_BYTES, ResourceTargetView, ResourceTargetsApplied,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientError};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The schema of the documents [`document`] builds.
const DOCUMENT_SCHEMA: &str = "actingcommand.resource-targets.v2";
/// The longest validity [`valid_until`] gives a policy, in days.
pub const MAX_VALID_DAYS: u64 = 365;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// The key table's bound of a `pointer` value (`json_pointer`).
const MAX_POINTER_BYTES: usize = 512;

/// `targets`: what the instance `alias` can target and the policy it holds.
pub fn view(client: &RuntimeClient, alias: &str) -> Result<ResourceTargetView, RuntimeClientError> {
    client.resource_target_view(alias)
}

/// The members every answer of `targets` starts with.
fn view_header(view: &ResourceTargetView) -> Map<String, Value> {
    let mut header = Map::new();
    header.insert("instance_alias".to_owned(), json!(view.instance_alias));
    header.insert("policy_instance".to_owned(), json!(view.policy_instance));
    header.insert(
        "evaluated_at_unix_ms".to_owned(),
        json!(view.evaluated_at_unix_ms),
    );
    header.insert(
        "as_of_ledger_position".to_owned(),
        json!(view.as_of_ledger_position),
    );
    if let Some(catalog_hash) = &view.catalog_hash {
        header.insert("catalog_hash".to_owned(), json!(catalog_hash));
    }
    header
}

/// MCP `ac_resources_list`'s answer: the header, `targetable` and `not_targetable`.
pub fn resources_answer(view: &ResourceTargetView) -> Value {
    let mut result = view_header(view);
    result.insert("targetable".to_owned(), json!(view.targetable));
    result.insert("not_targetable".to_owned(), json!(view.not_targetable));
    Value::Object(result)
}

/// MCP `ac_targets_get`'s answer: the header and `active`.
pub fn active_answer(view: &ResourceTargetView) -> Value {
    let mut result = view_header(view);
    result.insert("active".to_owned(), json!(view.active));
    Value::Object(result)
}

/// `agent-apply-resource-targets --policy-file`: the document, bounded, not empty, UTF-8; the
/// Runtime parses and checks its content.
pub fn read_document(path: &Path) -> Result<String, InputProblem> {
    let bytes = read_bounded(path, MAX_RESOURCE_TARGETS_DOCUMENT_BYTES)?;
    if bytes.is_empty() {
        return Err(InputProblem::Empty);
    }
    String::from_utf8(bytes).map_err(InputProblem::NotUtf8)
}

/// The end of a validity of `days` from now, in Unix milliseconds; `None` when the system clock
/// is before the Unix epoch or out of range.
pub fn valid_until(days: u64) -> Option<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())?;
    Some(now + days * DAY_MS)
}

/// The policy document of `targets` (an array of v2 targets) for the instance `view` shows:
/// `{schema_version, instance, valid_until_unix_ms?, targets}`, `valid_until_unix_ms` only when
/// asked for (whether empty targets clear is the Runtime's answer). One larger than
/// `MAX_RESOURCE_TARGETS_DOCUMENT_BYTES` is `resource_targets_document_too_large` with its size.
pub fn document(
    view: &ResourceTargetView,
    valid_until_unix_ms: Option<u64>,
    targets: &[Value],
) -> Result<String, Outcome> {
    let mut document = Map::new();
    document.insert("schema_version".to_owned(), json!(DOCUMENT_SCHEMA));
    document.insert("instance".to_owned(), json!(view.policy_instance));
    if let Some(valid_until_unix_ms) = valid_until_unix_ms {
        document.insert("valid_until_unix_ms".to_owned(), json!(valid_until_unix_ms));
    }
    document.insert("targets".to_owned(), Value::Array(targets.to_vec()));
    let document = Value::Object(document).to_string();
    if document.len() > MAX_RESOURCE_TARGETS_DOCUMENT_BYTES {
        return Err(Outcome::new(ActingctlCode::ResourceTargetsDocumentTooLarge)
            .actual_bytes(document.len())
            .limit_bytes(MAX_RESOURCE_TARGETS_DOCUMENT_BYTES));
    }
    Ok(document)
}

/// `targets set`: the policy `document` through the formal entry; what the Runtime applied. A
/// refused document comes back as the error, its receipt carrying why ([`with_rejection`]).
pub fn apply(
    client: &RuntimeClient,
    document: String,
) -> Result<ResourceTargetsApplied, RuntimeClientError> {
    client.apply_resource_targets(document, None)
}

/// The answer of `targets set`: `{applied}`; a member that does not encode is the encoder's
/// error.
pub fn applied_answer(
    applied: &ResourceTargetsApplied,
) -> Result<Map<String, Value>, serde_json::Error> {
    let mut answer = Map::new();
    answer.insert("applied".to_owned(), serde_json::to_value(applied)?);
    Ok(answer)
}

/// `outcome`, the failure of a refused policy, with where and why the Runtime refused it. The
/// receipt's typed `resource_targets_rejection` is not in its outcome: it is added as the
/// registered values `line`, `column`, `resource_targets_rejection_reason` and `json_pointer`
/// (within the key table's bound). Without such a receipt `outcome` is unchanged.
pub fn with_rejection(outcome: Outcome, error: &RuntimeClientError) -> Outcome {
    let Some(rejection) = error
        .received_receipt()
        .and_then(|receipt| receipt.resource_targets_rejection())
    else {
        return outcome;
    };
    let outcome = outcome
        .line(rejection.line)
        .column(rejection.column)
        .resource_targets_rejection_reason(ResourceTargetsRejectionReason::of(rejection.reason));
    if rejection.field_path.len() <= MAX_POINTER_BYTES {
        outcome.json_pointer(rejection.field_path.as_str())
    } else {
        outcome
    }
}
