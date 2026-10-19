// SPDX-License-Identifier: AGPL-3.0-only

// #411 A21 (register §1.123; exec gate key 101): a failed `observe --capture` keeps
// `device_error` (exit 4, lab-recording.md) and carries the Runtime's answer in its details:
// `error` (the Runtime outcome's code), `state: "failed"`, the failed receipt's `req_id` and
// `receipt_state` when a receipt was received, `runtime_outcome`, the debug session's
// `correlation_id` and `hint`; MCP reads `req_id` from them. A `RuntimeClientError` carrying a
// receipt can be built only inside the Runtime client, so the details builder is tested from the
// parts of the error it reads: the error's outcome and its received receipt.

use super::*;
use actingcommand_contract::codes::LabErrorCode;
use actingcommand_contract::outcome::{Code, Location};
use actingcommand_contract::{
    EventActor, EventSource, IdentifierIssuer, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceiptState, RuntimeRequest,
};

#[test]
fn a21_a_failed_capture_observe_carries_req_id_and_the_runtime_outcome() {
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
    // The Runtime's failed observation: its kernel code on top, `recognition_failed` beneath.
    let outcome = RuntimeErrorProjection::new(RuntimeErrorCode::RecognitionFailed, false)
        .receipt_outcome(
            Some(Code::relayed("observation_fact_failed").expect("registered code")),
            Some(Location::relayed("encode_observation_fact").expect("registered location")),
        );
    let receipt =
        RuntimeReceipt::failure(&request, RuntimeReceiptState::Failed, None, outcome.clone())
            .expect("failed receipt");
    let correlation = request.correlation_id();

    for received in [Some(&receipt), None] {
        let error = runtime_observe_failure(
            "runtime client error".to_string(),
            &outcome,
            received,
            correlation,
        );
        assert_eq!(error.code, LabErrorCode::DeviceError.as_str());
        assert_eq!(error.class, ErrorKind::DeviceInstance);
        assert_eq!(error.exit_code(), 4);
        assert_eq!(error.message, "runtime client error");
        let details = error.details.expect("the failure carries details");
        assert_eq!(details["error"], "observation_fact_failed");
        assert_eq!(details["state"], "failed");
        assert_eq!(
            details["runtime_outcome"]["code"],
            "observation_fact_failed"
        );
        assert_eq!(details["correlation_id"], json!(correlation));
        assert_eq!(details["hint"], "inspect-runtime-ledger");
        match received {
            Some(receipt) => {
                assert_eq!(details["req_id"], json!(receipt.request_id()));
                assert_eq!(details["receipt_state"], "failed");
            }
            None => {
                assert!(details.get("req_id").is_none(), "{details}");
                assert!(details.get("receipt_state").is_none(), "{details}");
            }
        }
    }
}
