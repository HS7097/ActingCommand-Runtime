// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #433 LABDO-1 (board #433 6733194460; coordinator brief rd-1109/labdo-exit4): a Lab
//! operation the Runtime answered, whose runtime client check then failed, reaches an MCP caller
//! of ac_lab_do as actinglab answers it: never class device, with the req_id, actinglab's code,
//! its details (the effect the Runtime reported among them) and its message. The envelope is the
//! one actinglab's `lab_operation_check_failed` answer prints (exit 5).

use super::super::interpret;
use crate::child::Captured;
use crate::codes::ActingctlCode;
use actingcommand_contract::codes::ContractCode;
use serde_json::{Value, json};
use std::process::ExitStatus;

#[cfg(windows)]
fn exited(code: u8) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(u32::from(code))
}

#[cfg(unix)]
fn exited(code: u8) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(i32::from(code) << 8)
}

const REQUEST: &str = "request_0123456789abcdef0123456789abcdef";
const CHECK_CODE: &str = "lab_operation_check_failed";

#[test]
fn lab_do_post_receipt_check_failure_passes_through_mcp() {
    let client_message = "runtime client error runtime_lab_diagnostic_budget_mismatch during \
                          verify_contained_lab_operation";
    let message = format!(
        "the Runtime answered the Lab operation and reports its effect as performed; a check of \
         the client after the receipt then failed, so do not send it again as though it had not \
         run: {client_message}"
    );
    let details = json!({
        "req_id": REQUEST,
        "correlation_id": "correlation_fedcba9876543210fedcba9876543210",
        "receipt_state": "completed",
        "effect": "performed",
        "executed": true,
        "failure": null,
        "client_code": "runtime_lab_diagnostic_budget_mismatch",
        "client_message": client_message,
    });
    let envelope = json!({
        "schema_version": "0.2",
        "ok": false,
        "command": "do",
        "error": {
            "code": CHECK_CODE,
            "message": message,
            "blocked_by": [],
            "details": details,
        },
    });
    let answer = interpret(
        &Captured {
            status: exited(5),
            stdout: envelope.to_string().into_bytes(),
            stderr: Vec::new(),
        },
        "do",
    );
    assert!(answer.returned, "the child ran to its end");
    assert_eq!(answer.req_id.as_deref(), Some(REQUEST));
    let error = match answer.result {
        Ok(result) => panic!("a failed check answered {result}"),
        Err(error) => error,
    };
    let shown = error.to_value();
    assert_eq!(error.class(), "runtime", "{shown}");
    assert_ne!(error.class(), "device", "{shown}");
    let outcome = error.outcome_ref();
    assert!(outcome.is(ActingctlCode::LabToolFailed), "{shown}");
    assert_eq!(
        Some(outcome.values()),
        json!({"exit_code": 5, "req_id": REQUEST}).as_object(),
        "{shown}"
    );
    let causes = outcome.causes();
    assert_eq!(causes.len(), 3, "{shown}");
    assert_eq!(causes[0].outcome().code().as_str(), CHECK_CODE, "{shown}");
    for link in &causes[1..] {
        assert_eq!(
            link.outcome().code().as_str(),
            ContractCode::ForeignOtherError.as_str(),
            "{shown}"
        );
    }
    let details_text = causes[1]
        .outcome()
        .value("raw_text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the details are not relayed: {shown}"));
    let relayed = serde_json::from_str::<Value>(details_text)
        .unwrap_or_else(|error| panic!("the details are not JSON ({error}): {details_text}"));
    assert_eq!(relayed, details, "{shown}");
    assert_eq!(relayed["effect"], json!("performed"), "{shown}");
    assert_eq!(
        Some(causes[2].outcome().values()),
        json!({"raw_source": "process", "raw_text": message, "exit_code": 5}).as_object(),
        "actinglab's message is not kept: {shown}"
    );
}
