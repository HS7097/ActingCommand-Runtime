// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A4 (model section 7.2): `RuntimeClientError::outcome()` puts a denied or failed
//! receipt's outcome on top, else the client's own code, and the client's relations become links.

use crate::{ClientCode, ClientLocation, RuntimeClientError};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{CauseRelation, Code};
use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use serde_json::json;

fn health_request(ids: &IdentifierIssuer) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::Health,
    )
    .expect("health request")
}

/// A denied receipt's outcome is the error's outcome, unchanged: the Runtime's specific code on
/// top with the Runtime error code and the operation it failed at.
#[test]
fn a4_a_rejected_request_answers_with_the_receipt_outcome_on_top() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = health_request(&ids);
    let projection = RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false);
    let receipt = RuntimeReceipt::failure(
        &request,
        RuntimeReceiptState::Denied,
        None,
        projection.receipt_outcome(
            Some(SharedCode::ScheduledContainedTaskNotClientCancellable.code()),
            Some(ContractLocation::CancelContainedTask.location()),
        ),
    )
    .expect("denied receipt");
    let error = RuntimeClientError::rejected(
        ContractLocation::CancelContainedTask.location(),
        receipt.error_projection().expect("denied projection"),
        receipt.outcome().clone(),
    )
    .with_received_receipt(receipt.clone());

    let outcome = error.outcome();
    assert_eq!(&outcome, receipt.outcome());
    assert_eq!(
        outcome.code().as_str(),
        "scheduled_contained_task_not_client_cancellable"
    );
    assert_eq!(
        outcome.value("runtime_code"),
        Some(&json!("invalid_request"))
    );
    assert_eq!(
        outcome.value("operation"),
        Some(&json!("cancel_contained_task"))
    );
    assert!(outcome.causes().is_empty());
    assert_eq!(error.projection(), Some(&projection));
    assert_eq!(
        error.host_failure(),
        Some((
            "scheduled_contained_task_not_client_cancellable",
            "cancel_contained_task"
        ))
    );
}

/// Without a receipt the client's own code is the top, with its operation; the failed
/// receipt-header read is a `foreign_os_error` `caused_by` link with the I/O kind and the
/// system's text.
#[test]
fn a4_a_client_failure_answers_with_its_code_and_the_header_read_as_a_foreign_link() {
    let read = std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "receipt header read timed out",
    );
    let error = RuntimeClientError::fatal(
        ClientCode::RuntimeReceiptHeaderFailed,
        ContractLocation::RunContainedTask.location(),
    )
    .with_receipt_header_io(&read);

    let outcome = error.outcome();
    assert!(outcome.is(ClientCode::RuntimeReceiptHeaderFailed));
    assert_eq!(
        outcome.value("operation"),
        Some(&json!(ContractLocation::RunContainedTask.as_str()))
    );
    let [link] = outcome.causes() else {
        panic!("one link: {outcome}");
    };
    assert_eq!(link.relation(), &CauseRelation::CausedBy);
    assert_eq!(link.outcome().code().as_str(), "foreign_os_error");
    assert_eq!(link.outcome().value("raw_source"), Some(&json!("os")));
    assert_eq!(link.outcome().value("io_kind"), Some(&json!("timed_out")));
    assert_eq!(link.outcome().value("io_op"), Some(&json!("read")));
    assert_eq!(
        link.outcome().value("raw_text"),
        Some(&json!("receipt header read timed out"))
    );
}

/// A related failure is a `related` link; once the request's terminal receipt was committed, it
/// is an `after_commit` link under the client's code.
#[test]
fn a4_related_failures_become_related_or_after_commit_links() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let operation = ClientLocation::RuntimeHealth.location();
    let related = RuntimeClientError::fatal(ClientCode::RuntimeReceiptTimeout, operation);
    let expected_link = |code: Code| {
        json!({
            "code": code.as_str(),
            "operation": ClientLocation::RuntimeHealth.as_str(),
        })
    };

    let error = RuntimeClientError::fatal(ClientCode::RuntimeResultUnexpected, operation)
        .with_related(related.clone());
    let outcome = error.outcome();
    assert!(outcome.is(ClientCode::RuntimeResultUnexpected));
    let [link] = outcome.causes() else {
        panic!("one link: {outcome}");
    };
    assert_eq!(link.relation(), &CauseRelation::Related);
    assert_eq!(
        json!({
            "code": link.outcome().code().as_str(),
            "operation": link.outcome().value("operation"),
        }),
        expected_link(ClientCode::RuntimeReceiptTimeout.code())
    );

    let request = health_request(&ids);
    let committed = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::Health {
            owner_epoch: *ids.mint_owner_epoch().expect("owner epoch").transport(),
        },
    )
    .expect("completed receipt");
    let error = RuntimeClientError::after_commit(
        ClientCode::RuntimeResultUnexpected,
        operation,
        committed,
        related,
    );
    let outcome = error.outcome();
    assert!(outcome.is(ClientCode::RuntimeResultUnexpected));
    let [link] = outcome.causes() else {
        panic!("one link: {outcome}");
    };
    assert_eq!(link.relation(), &CauseRelation::AfterCommit);
    assert!(link.outcome().is(ClientCode::RuntimeReceiptTimeout));
}
