// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A2a, part 1 (brief-a2a.md, test T5): the client error text names the Runtime's
//! code and the receipt state by their registered spellings, never by their Rust names.

use crate::{ClientLocation, RuntimeClientError};
use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
};

/// #378 A2a T5 (brief-a2a.md; model-378 §2.2 :90): a rejected request whose received receipt
/// failed with `lease_busy` reads `runtime code lease_busy` and `state=failed` in the client
/// error text, not `LeaseBusy` or `Failed`.
#[test]
fn gate_client_error_text_names_the_runtime_code_and_receipt_state_by_registered_spelling() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::Health,
    )
    .expect("request");
    let projection = RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false);
    let receipt = RuntimeReceipt::error(
        &request,
        RuntimeReceiptState::Failed,
        None,
        projection.clone(),
    )
    .expect("failed receipt");
    let error = RuntimeClientError::rejected(
        ClientLocation::RuntimeHealth,
        projection,
        receipt.outcome().clone(),
    )
    .with_received_receipt(receipt);
    let text = error.to_string();
    assert!(text.contains("runtime code lease_busy"), "{text}");
    assert!(text.contains("state=failed"), "{text}");
    assert!(!text.contains("LeaseBusy"), "{text}");
    assert!(!text.contains("Failed"), "{text}");
}
