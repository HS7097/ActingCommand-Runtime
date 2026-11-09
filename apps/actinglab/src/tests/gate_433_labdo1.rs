// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #433 LABDO-1 (board #433 6733194460; coordinator brief rd-1109/labdo-exit4): when the
// Runtime answered a Lab operation and a check of the runtime client after its receipt failed,
// the operation ran as the receipt reports. Online `do` then answers `lab_operation_check_failed`
// with the receipt's req_id and correlation_id, the effect its record reports and the client's
// own code and message, exit 5, never `device_error` (exit 4): a caller that read exit 4 as
// "nothing happened" and sent the operation again acted twice. Every expected value comes from
// the slice's requirement; no Runtime, clock or device is involved.

use super::*;
use actingcommand_contract::codes::LabErrorCode;
use actingcommand_contract::{CorrelationId, RuntimeReceiptState};
use actingcommand_runtime_client::LabOperationAnswer;
use actingcommand_runtime_client::test_support::lab_operation_check_failure;

const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CORRELATION: &str = "correlation_fedcba9876543210fedcba9876543210";

fn answer(effect: Option<EffectDisposition>) -> LabOperationAnswer {
    LabOperationAnswer {
        request_id: serde_json::from_value(json!(REQUEST)).expect("a canonical request id"),
        correlation_id: serde_json::from_value::<CorrelationId>(json!(CORRELATION))
            .expect("a canonical correlation id"),
        receipt_state: RuntimeReceiptState::Completed,
        effect,
        failure: None,
    }
}

/// A fake runtime client whose receipt succeeded and whose evidence check after it failed: the
/// result `run_contained_lab_operation` returns then, the Runtime's answer on the error.
fn fake_client(
    effect: Option<EffectDisposition>,
) -> Result<actingcommand_runtime_client::VerifiedLabOperation, RuntimeClientError> {
    Err(lab_operation_check_failure(
        RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch,
        answer(effect),
    ))
}

#[test]
fn lab_do_post_receipt_check_failure_names_effect() {
    let client_code = RuntimeContractCode::RuntimeLabDiagnosticBudgetMismatch.as_str();
    let cases = [
        (
            Some(EffectDisposition::Performed),
            json!("performed"),
            json!(true),
        ),
        (
            Some(EffectDisposition::NotPerformed),
            json!("not_performed"),
            json!(false),
        ),
        (
            Some(EffectDisposition::Indeterminate),
            json!("indeterminate"),
            Value::Null,
        ),
        (None, Value::Null, Value::Null),
    ];
    for (effect, effect_value, executed) in cases {
        let error = match fake_client(effect).map_err(lab_operation_call_error) {
            Ok(_) => panic!("{effect:?}: a failed client check answered success"),
            Err(error) => error,
        };
        assert_eq!(
            error.code,
            crate::codes::ActinglabCode::LabOperationCheckFailed.as_str(),
            "{effect:?}"
        );
        assert_ne!(error.code, LabErrorCode::DeviceError.as_str(), "{effect:?}");
        assert_eq!(error.class, ErrorKind::RuntimeUnavailable, "{effect:?}");
        assert_eq!(error.exit_code(), 5, "{effect:?}");
        let details = error
            .details
            .as_ref()
            .unwrap_or_else(|| panic!("{effect:?}: the answer has no details"));
        assert_eq!(details["req_id"], json!(REQUEST), "{effect:?}: {details}");
        assert_eq!(
            details["correlation_id"],
            json!(CORRELATION),
            "{effect:?}: {details}"
        );
        assert_eq!(
            details["receipt_state"],
            json!("completed"),
            "{effect:?}: {details}"
        );
        assert_eq!(details["effect"], effect_value, "{effect:?}: {details}");
        assert_eq!(details["executed"], executed, "{effect:?}: {details}");
        assert_eq!(details["failure"], Value::Null, "{effect:?}: {details}");
        assert_eq!(
            details["client_code"],
            json!(client_code),
            "{effect:?}: {details}"
        );
        let client_message = details["client_message"]
            .as_str()
            .unwrap_or_else(|| panic!("{effect:?}: no client_message: {details}"));
        assert!(
            client_message.contains(client_code),
            "{effect:?}: the client's message does not name its code: {client_message}"
        );
        assert!(
            error.message.contains(client_message),
            "{effect:?}: the answer's message drops the client's: {}",
            error.message
        );
    }
}
