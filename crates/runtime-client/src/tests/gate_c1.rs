// SPDX-License-Identifier: AGPL-3.0-only

//! #382 C-1 (test plan C-1). G6, inventory key R3: the result class of a failed client call
//! (Workflow #338 R6). G3, inventory key R1: the receipt waits of existing operations, by their
//! contract values.

use crate::client::{
    INSTANCE_DISCOVERY_RESPONSE_TIMEOUT, contained_task_response_timeout,
    pause_scheduling_response_timeout, receipt_response_timeout,
};
use crate::{RuntimeClientConfig, RuntimeClientError, RuntimeClientErrorClass};
use actingcommand_contract::{
    ApplicationLifecycleAction, ArtifactKind, ArtifactMediaType, ArtifactProducer,
    ArtifactRedactionState, EventActor, EventSource, GovernanceIdentityCard, IdentifierIssuer,
    ProjectedArtifactReference, RetentionClass, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    SavedArtifactOcrRequest, SavedArtifactOcrSource, SchedulingPauseScope, TerminalEvent,
};
use std::time::Duration;

/// Workflow #338 R6: the class of each Runtime error code. No wildcard: a new code must be
/// classified here.
fn r6_class(code: RuntimeErrorCode) -> RuntimeClientErrorClass {
    use RuntimeClientErrorClass::{Device, Runtime, Safety, Usage};
    match code {
        RuntimeErrorCode::InvalidRequest
        | RuntimeErrorCode::InstanceUnknown
        | RuntimeErrorCode::PackageInvalid
        | RuntimeErrorCode::ReadonlyCapabilityInvalid => Usage,
        RuntimeErrorCode::LeaseBusy
        | RuntimeErrorCode::LeaseCooldown
        | RuntimeErrorCode::LeaseExpired
        | RuntimeErrorCode::LeaseMissing
        | RuntimeErrorCode::LeaseMismatch
        | RuntimeErrorCode::HolderMismatch
        | RuntimeErrorCode::ConnectionMismatch
        | RuntimeErrorCode::InstanceMismatch
        | RuntimeErrorCode::QueueFull
        | RuntimeErrorCode::QueueExpired
        | RuntimeErrorCode::QueueMissing
        | RuntimeErrorCode::QueueConnectionMismatch
        | RuntimeErrorCode::TransferNotSafe
        | RuntimeErrorCode::ContainedTaskBusy
        | RuntimeErrorCode::RuntimeBusy => Safety,
        RuntimeErrorCode::CaptureFailed
        | RuntimeErrorCode::RecognitionFailed
        | RuntimeErrorCode::BackendOpenFailed
        | RuntimeErrorCode::BackendOperationFailed
        | RuntimeErrorCode::ContainedTaskDeadlineExceeded
        | RuntimeErrorCode::ContainedTaskCancelled
        | RuntimeErrorCode::ContainedTaskPaused => Device,
        RuntimeErrorCode::RuntimeUnavailable
        | RuntimeErrorCode::RuntimeFatal
        | RuntimeErrorCode::OwnerConflict
        | RuntimeErrorCode::StaleOwnerEpoch
        | RuntimeErrorCode::LedgerFailure
        | RuntimeErrorCode::RuntimeOwnerMismatch
        | RuntimeErrorCode::ProtocolInvalid
        | RuntimeErrorCode::EvidenceExportFailed => Runtime,
    }
}

#[test]
fn gate_every_runtime_error_code_has_its_r6_disposition() {
    for code in [
        RuntimeErrorCode::RuntimeBusy,
        RuntimeErrorCode::RuntimeOwnerMismatch,
        RuntimeErrorCode::InvalidRequest,
        RuntimeErrorCode::RuntimeUnavailable,
        RuntimeErrorCode::RuntimeFatal,
        RuntimeErrorCode::OwnerConflict,
        RuntimeErrorCode::ProtocolInvalid,
        RuntimeErrorCode::InstanceUnknown,
        RuntimeErrorCode::LeaseBusy,
        RuntimeErrorCode::LeaseCooldown,
        RuntimeErrorCode::LeaseExpired,
        RuntimeErrorCode::LeaseMissing,
        RuntimeErrorCode::StaleOwnerEpoch,
        RuntimeErrorCode::LeaseMismatch,
        RuntimeErrorCode::QueueFull,
        RuntimeErrorCode::QueueExpired,
        RuntimeErrorCode::QueueMissing,
        RuntimeErrorCode::QueueConnectionMismatch,
        RuntimeErrorCode::TransferNotSafe,
        RuntimeErrorCode::InstanceMismatch,
        RuntimeErrorCode::HolderMismatch,
        RuntimeErrorCode::ConnectionMismatch,
        RuntimeErrorCode::ReadonlyCapabilityInvalid,
        RuntimeErrorCode::CaptureFailed,
        RuntimeErrorCode::RecognitionFailed,
        RuntimeErrorCode::BackendOpenFailed,
        RuntimeErrorCode::BackendOperationFailed,
        RuntimeErrorCode::PackageInvalid,
        RuntimeErrorCode::EvidenceExportFailed,
        RuntimeErrorCode::LedgerFailure,
        RuntimeErrorCode::ContainedTaskDeadlineExceeded,
        RuntimeErrorCode::ContainedTaskBusy,
        RuntimeErrorCode::ContainedTaskCancelled,
        RuntimeErrorCode::ContainedTaskPaused,
    ] {
        let error = RuntimeClientError::rejected(
            "run_contained_task",
            RuntimeErrorProjection::new(code, false),
        );
        assert_eq!(error.disposition(), r6_class(code), "{code:?}");
    }
}

#[test]
fn gate_failures_after_send_without_a_projection_are_uncertain() {
    for (code, operation) in [
        // The receipt timed out.
        ("runtime_receipt_timeout", "run_contained_task"),
        // The receipt was not read or not confirmed.
        ("runtime_receipt_header_failed", "install_transition"),
        ("runtime_receipt_read_failed", "request_runtime_shutdown"),
        (
            "install_transition_receipt_unconfirmed",
            "install_transition",
        ),
        (
            "runtime_shutdown_receipt_unconfirmed",
            "request_runtime_shutdown",
        ),
        ("runtime_scheduling_resume_unconfirmed", "resume_scheduling"),
        // The owner epoch changed while the request was out.
        (
            "runtime_owner_epoch_changed",
            "recover_contained_task_timeout",
        ),
        ("runtime_owner_epoch_changed", "resume_scheduling"),
        // `runtime_contained_task_recovery_*`.
        (
            "runtime_contained_task_recovery_required",
            "cancel_contained_task_and_reset",
        ),
        (
            "runtime_contained_task_recovery_failed",
            "recover_contained_task_timeout",
        ),
    ] {
        assert_eq!(
            RuntimeClientError::fatal(code, operation).disposition(),
            RuntimeClientErrorClass::Uncertain,
            "{code} during {operation}"
        );
    }

    // The same epoch change found while connecting, before any request was sent.
    assert_ne!(
        RuntimeClientError::fatal("runtime_owner_epoch_changed", "connect_runtime").disposition(),
        RuntimeClientErrorClass::Uncertain
    );
}

#[test]
fn gate_a_failure_chained_to_an_after_send_failure_is_uncertain() {
    // The chain `run_contained_task` raises when the receipt of a contained run times out.
    let error = RuntimeClientError::fatal(
        "runtime_contained_task_response_timeout",
        "run_contained_task",
    )
    .with_related(RuntimeClientError::fatal(
        "runtime_receipt_timeout",
        "run_contained_task",
    ));
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
}

#[test]
fn gate_settled_failures_are_not_uncertain() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::Health,
    )
    .expect("request");
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::Health {
            owner_epoch: *ids.mint_owner_epoch().expect("epoch").transport(),
        },
    )
    .expect("receipt");
    let after_commit = RuntimeClientError::after_commit(
        "runtime_projection_failed_after_terminal",
        "query_runtime_flow_projection",
        receipt,
        RuntimeClientError::fatal("runtime_receipt_timeout", "query_runtime_events"),
    );
    assert_ne!(
        after_commit.disposition(),
        RuntimeClientErrorClass::Uncertain,
        "a committed terminal receipt settles its request"
    );

    // Raised with the dropped request's own failure as its related failure: the same owner,
    // reached again, shows the request was not performed.
    let not_performed =
        RuntimeClientError::fatal("runtime_operation_unsupported", "resume_scheduling")
            .with_related(RuntimeClientError::fatal(
                "runtime_receipt_header_failed",
                "resume_scheduling",
            ));
    assert_ne!(
        not_performed.disposition(),
        RuntimeClientErrorClass::Uncertain,
        "runtime_operation_unsupported settles its request"
    );
}

/// A saved-artifact OCR request; the receipt wait depends only on the operation kind.
fn recognize_artifact(ids: &IdentifierIssuer) -> RuntimeOperation {
    let frame_id = *ids.mint_frame_id().expect("frame id").transport();
    let event = |sequence| TerminalEvent {
        sequence,
        event_id: *ids.mint_event_id().expect("event id").transport(),
    };
    RuntimeOperation::RecognizeArtifact {
        request: Box::new(SavedArtifactOcrRequest {
            source: SavedArtifactOcrSource {
                state_root: "C:\\state".to_owned(),
                through_sequence: 3,
                frame_id,
                artifact: ProjectedArtifactReference {
                    artifact_id: *ids.mint_artifact_id().expect("artifact id").transport(),
                    kind: ArtifactKind::CaptureFrame,
                    run_id: None,
                    frame_id: Some(frame_id),
                    correlation_id: None,
                    object_key: None,
                    media_type: ArtifactMediaType::ImagePng,
                    byte_count: 4096,
                    sha256: "0".repeat(64),
                    created_at_unix_ms: 1_760_000_000_000,
                    producer: ArtifactProducer::CapturePipeline,
                    retention_class: RetentionClass::Adaptive,
                    redaction_state: ArtifactRedactionState::NotRequired,
                },
                created: event(1),
                verified: event(2),
                captured: event(3),
            },
            package_path: "C:\\packages\\saved-ocr.zip".to_owned(),
            expected_sha256: "0".repeat(64).into(),
            target_id: "fixture/ocr".to_owned(),
        }),
    }
}

#[test]
fn gate_documented_operations_wait_their_contract_receipt_budget() {
    let io = Duration::from_millis(100);
    let backend_open = Duration::from_secs(2);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let holder_id = *ids.mint_holder_id().expect("holder id").transport();
    for (row, operation, expected) in [
        // saved-artifact-ocr.md: the 120-second request deadline plus the receipt I/O allowance.
        (
            "recognize_artifact",
            recognize_artifact(&ids),
            Duration::from_millis(120_000) + io,
        ),
        // scheduling-pause.md: 30 000 ms backend-open bound plus 3 000 ms claim wait plus io.
        (
            "self_check_instance",
            RuntimeOperation::SelfCheckInstance {
                instance_alias: "node-01".to_owned(),
            },
            Duration::from_millis(33_000) + io,
        ),
        (
            "instance resume",
            RuntimeOperation::ResumeScheduling {
                scope: SchedulingPauseScope::Instance {
                    instance_alias: "node-01".to_owned(),
                },
                expected: None,
            },
            Duration::from_millis(33_000) + io,
        ),
        // scheduling-pause.md: a global resume keeps the IO timeout.
        (
            "global resume",
            RuntimeOperation::ResumeScheduling {
                scope: SchedulingPauseScope::Global,
                expected: None,
            },
            io,
        ),
        // input-receipt-wait.md: an ApplicationLifecycle request waits `I` plus 3 s.
        (
            "application_lifecycle",
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: "node-01".to_owned(),
                holder_id,
                action: ApplicationLifecycleAction::Launch,
            },
            io + Duration::from_millis(3_000),
        ),
        // No budget of their own: the IPC read/write timeout.
        ("status", RuntimeOperation::Status, io),
        (
            "declare_governance_identity",
            RuntimeOperation::DeclareGovernanceIdentity {
                card: GovernanceIdentityCard {
                    client: "gate-c1".to_owned(),
                    client_version: None,
                    instance: None,
                },
            },
            io,
        ),
    ] {
        assert_eq!(
            receipt_response_timeout(&operation, io, backend_open),
            Ok(expected),
            "{row}"
        );
    }

    // The frozen IPC read/write timeout is 5 000 ms.
    assert_eq!(
        RuntimeClientConfig::new("C:\\state", EventActor::Cli, EventSource::Cli).io_timeout(),
        Duration::from_millis(5_000)
    );
}

#[test]
fn gate_instance_discovery_waits_25_s_for_its_receipt() {
    // emulator-control.md: `discover_instances()` waits 25 s for the receipt.
    assert_eq!(INSTANCE_DISCOVERY_RESPONSE_TIMEOUT, Duration::from_secs(25));
}

#[test]
fn gate_a_contained_task_receipt_wait_covers_its_deadline() {
    // contracts/README.md: the default task deadline 60 000 ms and the task RPC ceiling
    // 1 800 000 ms; the receipt wait follows the task's deadline, not the IO exchange.
    let io = Duration::from_millis(100);
    for deadline_ms in [60_000_u64, 1_800_000] {
        let deadline = Duration::from_millis(deadline_ms);
        let wait = contained_task_response_timeout(io, deadline).expect("the receipt wait");
        assert!(wait >= deadline, "{deadline_ms} ms: {wait:?}");
    }
}

#[test]
fn gate_a_pause_waits_its_drain_plus_30_s_grace_plus_io() {
    // scheduling-pause.md: the drain timeout (1 000..=600 000 ms) plus the 30 000 ms checkpoint
    // grace plus the IO timeout.
    let io = Duration::from_millis(100);
    for drain_timeout_ms in [1_000_u64, 600_000] {
        assert_eq!(
            pause_scheduling_response_timeout(drain_timeout_ms, io),
            Ok(Duration::from_millis(drain_timeout_ms + 30_000) + io),
            "drain {drain_timeout_ms} ms"
        );
    }
}
