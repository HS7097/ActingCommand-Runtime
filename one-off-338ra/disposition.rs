// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 Ra R6 evidence. The one-off workflow copies this file
// to crates/runtime-client/src/oneoff_338ra.rs and declares it from lib.rs under cfg(test).

use crate::{RuntimeClientError, RuntimeClientErrorClass};
use actingcommand_contract::{
    ContainedTaskRequest, EventActor, EventSource, IdentifierIssuer, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, TaskOutcome,
};
use std::collections::BTreeSet;

/// Exhaustive: a new RuntimeErrorCode variant fails to compile here.
const fn ordinal(code: RuntimeErrorCode) -> usize {
    match code {
        RuntimeErrorCode::RuntimeBusy => 0,
        RuntimeErrorCode::RuntimeOwnerMismatch => 1,
        RuntimeErrorCode::InvalidRequest => 2,
        RuntimeErrorCode::RuntimeUnavailable => 3,
        RuntimeErrorCode::RuntimeFatal => 4,
        RuntimeErrorCode::OwnerConflict => 5,
        RuntimeErrorCode::ProtocolInvalid => 6,
        RuntimeErrorCode::InstanceUnknown => 7,
        RuntimeErrorCode::LeaseBusy => 8,
        RuntimeErrorCode::LeaseCooldown => 9,
        RuntimeErrorCode::LeaseExpired => 10,
        RuntimeErrorCode::LeaseMissing => 11,
        RuntimeErrorCode::StaleOwnerEpoch => 12,
        RuntimeErrorCode::LeaseMismatch => 13,
        RuntimeErrorCode::QueueFull => 14,
        RuntimeErrorCode::QueueExpired => 15,
        RuntimeErrorCode::QueueMissing => 16,
        RuntimeErrorCode::QueueConnectionMismatch => 17,
        RuntimeErrorCode::TransferNotSafe => 18,
        RuntimeErrorCode::InstanceMismatch => 19,
        RuntimeErrorCode::HolderMismatch => 20,
        RuntimeErrorCode::ConnectionMismatch => 21,
        RuntimeErrorCode::ReadonlyCapabilityInvalid => 22,
        RuntimeErrorCode::CaptureFailed => 23,
        RuntimeErrorCode::RecognitionFailed => 24,
        RuntimeErrorCode::BackendOpenFailed => 25,
        RuntimeErrorCode::BackendOperationFailed => 26,
        RuntimeErrorCode::PackageInvalid => 27,
        RuntimeErrorCode::EvidenceExportFailed => 28,
        RuntimeErrorCode::LedgerFailure => 29,
        RuntimeErrorCode::ContainedTaskDeadlineExceeded => 30,
        RuntimeErrorCode::ContainedTaskBusy => 31,
        RuntimeErrorCode::ContainedTaskCancelled => 32,
        RuntimeErrorCode::ContainedTaskPaused => 33,
    }
}

/// The R6 table of the frozen model 5978610792, row by row.
const MODEL_TABLE: [(RuntimeErrorCode, RuntimeClientErrorClass); 34] = {
    use RuntimeClientErrorClass::{Device, Runtime, Safety, Usage};
    [
        (RuntimeErrorCode::InvalidRequest, Usage),
        (RuntimeErrorCode::InstanceUnknown, Usage),
        (RuntimeErrorCode::PackageInvalid, Usage),
        (RuntimeErrorCode::ReadonlyCapabilityInvalid, Usage),
        (RuntimeErrorCode::LeaseBusy, Safety),
        (RuntimeErrorCode::LeaseCooldown, Safety),
        (RuntimeErrorCode::LeaseExpired, Safety),
        (RuntimeErrorCode::LeaseMissing, Safety),
        (RuntimeErrorCode::LeaseMismatch, Safety),
        (RuntimeErrorCode::HolderMismatch, Safety),
        (RuntimeErrorCode::ConnectionMismatch, Safety),
        (RuntimeErrorCode::InstanceMismatch, Safety),
        (RuntimeErrorCode::QueueFull, Safety),
        (RuntimeErrorCode::QueueExpired, Safety),
        (RuntimeErrorCode::QueueMissing, Safety),
        (RuntimeErrorCode::QueueConnectionMismatch, Safety),
        (RuntimeErrorCode::TransferNotSafe, Safety),
        (RuntimeErrorCode::ContainedTaskBusy, Safety),
        (RuntimeErrorCode::RuntimeBusy, Safety),
        (RuntimeErrorCode::CaptureFailed, Device),
        (RuntimeErrorCode::RecognitionFailed, Device),
        (RuntimeErrorCode::BackendOpenFailed, Device),
        (RuntimeErrorCode::BackendOperationFailed, Device),
        (RuntimeErrorCode::ContainedTaskDeadlineExceeded, Device),
        (RuntimeErrorCode::ContainedTaskCancelled, Device),
        (RuntimeErrorCode::ContainedTaskPaused, Device),
        (RuntimeErrorCode::RuntimeUnavailable, Runtime),
        (RuntimeErrorCode::RuntimeFatal, Runtime),
        (RuntimeErrorCode::OwnerConflict, Runtime),
        (RuntimeErrorCode::StaleOwnerEpoch, Runtime),
        (RuntimeErrorCode::LedgerFailure, Runtime),
        (RuntimeErrorCode::RuntimeOwnerMismatch, Runtime),
        (RuntimeErrorCode::ProtocolInvalid, Runtime),
        (RuntimeErrorCode::EvidenceExportFailed, Runtime),
    ]
};

fn committed_receipt() -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::run_contained_task(
            "node.a",
            ids.mint_holder_id().expect("holder id"),
            ContainedTaskRequest::new("C:\\fixture\\contained-task.zip", "0".repeat(64))
                .expect("contained task request"),
        ),
    )
    .expect("runtime request");
    RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::ContainedTaskCompleted {
            run_id: *ids.mint_run_id().expect("run id").transport(),
            task_id: *ids.mint_task_id().expect("task id").transport(),
            task_request_id: request.request_id(),
            response_deadline_monotonic_ms: Some(60_000),
            outcome: TaskOutcome::Success,
            final_page: Some("fixture/final".to_owned()),
            executed_steps: 1,
        },
    )
    .expect("committed receipt")
}

#[test]
fn oneoff_338ra_disposition_covers_every_runtime_error_code() {
    let mut ordinals = BTreeSet::new();
    for (code, expected) in MODEL_TABLE {
        assert!(ordinals.insert(ordinal(code)), "{code:?} listed twice");
        for fatal in [false, true] {
            let error = RuntimeClientError::rejected(
                "oneoff_338ra",
                RuntimeErrorProjection::new(code, fatal),
            );
            let class = error.disposition();
            println!(
                "RA|R6|runtime_code={code:?}|fatal={fatal}|disposition={}|model={}",
                class.as_str(),
                expected.as_str()
            );
            assert_eq!(class, expected, "{code:?}");
        }
    }
    assert_eq!(ordinals.len(), 34);
    assert_eq!(ordinals.iter().copied().collect::<Vec<_>>(), (0..34).collect::<Vec<_>>());
    println!("RA|R6|covered={} of 34 RuntimeErrorCode variants", ordinals.len());
}

#[test]
fn oneoff_338ra_disposition_uncertain_and_client_cases() {
    use RuntimeClientErrorClass::{Device, Runtime, Safety, Uncertain};
    let timeout = || {
        RuntimeClientError::fatal(
            "runtime_contained_task_response_timeout",
            "run_contained_task",
        )
        .with_related(RuntimeClientError::fatal(
            "runtime_receipt_timeout",
            "exchange_runtime_request",
        ))
    };
    let lease_busy = || {
        RuntimeClientError::rejected(
            "safe_reset",
            RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false),
        )
    };
    let cases: Vec<(&str, RuntimeClientError, RuntimeClientErrorClass)> = vec![
        (
            "receipt header read failed after send",
            RuntimeClientError::fatal("runtime_receipt_header_failed", "exchange_runtime_request"),
            Uncertain,
        ),
        (
            "receipt body read failed after send",
            RuntimeClientError::fatal("runtime_receipt_read_failed", "exchange_runtime_request"),
            Uncertain,
        ),
        (
            "receipt deadline elapsed",
            RuntimeClientError::fatal("runtime_receipt_timeout", "exchange_runtime_request"),
            Uncertain,
        ),
        (
            "receipt failed validation",
            RuntimeClientError::fatal("runtime_receipt_invalid", "runtime_status"),
            Uncertain,
        ),
        (
            "receipt identity mismatch",
            RuntimeClientError::fatal("runtime_receipt_identity_mismatch", "runtime_status"),
            Uncertain,
        ),
        (
            "receipt result unexpected",
            RuntimeClientError::fatal("runtime_result_unexpected", "runtime_status"),
            Uncertain,
        ),
        (
            "shutdown receipt unconfirmed",
            RuntimeClientError::fatal(
                "runtime_shutdown_receipt_unconfirmed",
                "request_runtime_shutdown",
            ),
            Uncertain,
        ),
        (
            "contained task receipt timeout (submitter)",
            timeout(),
            Uncertain,
        ),
        (
            "contained task timeout, recovery busy on top",
            lease_busy().with_related(timeout()),
            Uncertain,
        ),
        (
            "owner epoch changed during recovery",
            RuntimeClientError::fatal(
                "runtime_owner_epoch_changed",
                "recover_contained_task_timeout",
            ),
            Uncertain,
        ),
        (
            "recovery failed",
            RuntimeClientError::fatal(
                "runtime_contained_task_recovery_failed",
                "recover_contained_task_timeout",
            ),
            Uncertain,
        ),
        (
            "recovery required (cancel_contained_task_and_reset)",
            RuntimeClientError::fatal(
                "runtime_contained_task_recovery_required",
                "cancel_contained_task_and_reset",
            ),
            Uncertain,
        ),
        (
            "owner epoch changed at connect (nothing sent)",
            RuntimeClientError::fatal("runtime_owner_epoch_changed", "connect_runtime"),
            Runtime,
        ),
        (
            "connect failed (nothing sent)",
            RuntimeClientError::fatal("runtime_connect_failed", "connect_runtime"),
            Runtime,
        ),
        (
            "request write failed",
            RuntimeClientError::fatal("runtime_request_write_failed", "exchange_runtime_request"),
            Runtime,
        ),
        (
            "committed terminal, projection read then timed out",
            RuntimeClientError::after_commit(
                "runtime_projection_failed_after_terminal",
                "query_runtime_flow_projection",
                committed_receipt(),
                RuntimeClientError::fatal("runtime_receipt_timeout", "exchange_runtime_request"),
            ),
            Runtime,
        ),
        (
            "contained run cancelled by a client (received receipt)",
            RuntimeClientError::fatal("runtime_contained_task_cancelled", "run_contained_task"),
            Device,
        ),
        (
            "contained run paused (received receipt)",
            RuntimeClientError::fatal("runtime_contained_task_paused", "run_contained_task"),
            Device,
        ),
        (
            "contained run deadline exceeded (received receipt)",
            RuntimeClientError::fatal(
                "runtime_contained_task_response_timeout",
                "run_contained_task",
            ),
            Device,
        ),
        (
            "cancelled run, reset refused busy on top",
            lease_busy().with_related(RuntimeClientError::fatal(
                "runtime_contained_task_cancelled",
                "run_contained_task",
            )),
            Safety,
        ),
        (
            "scheduled run not client cancellable (InvalidRequest)",
            RuntimeClientError::rejected(
                "cancel_contained_task",
                RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
                    .with_host_failure(
                        "scheduled_contained_task_not_client_cancellable",
                        "cancel_contained_task",
                    ),
            ),
            RuntimeClientErrorClass::Usage,
        ),
    ];
    for (label, error, expected) in cases {
        let class = error.disposition();
        println!(
            "RA|R6|case={label}|code={}|disposition={}|expected={}",
            error.code(),
            class.as_str(),
            expected.as_str()
        );
        assert_eq!(class, expected, "{label}");
    }
}
