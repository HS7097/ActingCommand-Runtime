// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A2a, part 1 (brief-a2a.md, tests T2 and T3): the closed diagnostic codes travel
//! as registered contract codes, and a scheduling pause carries its reason as `pause_reason`.

use actingcommand_contract::{
    DiagnosticCode, EventActor, EventSource, IdentifierIssuer, InstancePauseState,
    RuntimeOperation, RuntimeRequest, SchedulingPauseState,
};
use serde_json::{Value, json};

/// Every `DiagnosticCode` variant with its registered spelling (the D4 map of brief-a2a.md) and
/// its dotted spelling before A2a.
const DIAGNOSTIC_CODES: [(DiagnosticCode, &str, &str); 25] = [
    (
        DiagnosticCode::ProviderStartupFailed,
        "provider_startup_failed",
        "provider_startup_failed",
    ),
    (
        DiagnosticCode::RuntimeDiagnostic,
        "runtime_diagnostic",
        "runtime.diagnostic",
    ),
    (
        DiagnosticCode::RuntimeOwnerConflict,
        "owner_conflict",
        "runtime.owner_conflict",
    ),
    (
        DiagnosticCode::RuntimeProtocolInvalid,
        "protocol_invalid",
        "runtime.protocol_invalid",
    ),
    (DiagnosticCode::LeaseBusy, "lease_busy", "lease.busy"),
    (
        DiagnosticCode::LeaseCooldown,
        "lease_cooldown",
        "lease.cooldown",
    ),
    (
        DiagnosticCode::LeaseExpired,
        "lease_expired",
        "lease.expired",
    ),
    (
        DiagnosticCode::LeaseFencingDenied,
        "lease_fencing_denied",
        "lease.fencing_denied",
    ),
    (
        DiagnosticCode::LeaseQueueCancelled,
        "lease_queue_cancelled",
        "lease.queue_cancelled",
    ),
    (
        DiagnosticCode::LeaseQueueExpired,
        "lease_queue_expired",
        "lease.queue_expired",
    ),
    (
        DiagnosticCode::LeaseQueueDisconnected,
        "lease_queue_disconnected",
        "lease.queue_disconnected",
    ),
    (
        DiagnosticCode::BackendOpenFailed,
        "backend_open_failed",
        "backend.open_failed",
    ),
    (
        DiagnosticCode::BackendOperationFailed,
        "backend_operation_failed",
        "backend.operation_failed",
    ),
    (
        DiagnosticCode::CaptureFailed,
        "capture_failed",
        "capture.failed",
    ),
    (
        DiagnosticCode::ArtifactWriteFailed,
        "artifact_write_failed",
        "artifact.write_failed",
    ),
    (
        DiagnosticCode::ArtifactVerifyFailed,
        "artifact_verify_failed",
        "artifact.verify_failed",
    ),
    (
        DiagnosticCode::ArtifactExportFailed,
        "artifact_export_failed",
        "artifact.export_failed",
    ),
    (
        DiagnosticCode::PinnedFrameMissing,
        "artifact_pinned_frame_missing",
        "artifact.pinned_frame_missing",
    ),
    (
        DiagnosticCode::RecognitionFailed,
        "recognition_failed",
        "recognition.failed",
    ),
    (DiagnosticCode::InputFailed, "input_failed", "input.failed"),
    (
        DiagnosticCode::ApplicationFailed,
        "application_failed",
        "application.failed",
    ),
    (
        DiagnosticCode::CommandRejected,
        "command_rejected",
        "command.rejected",
    ),
    (
        DiagnosticCode::PolicyRejected,
        "policy_rejected",
        "policy.rejected",
    ),
    (
        DiagnosticCode::CatalogTransitionFailed,
        "catalog_transition_failed",
        "catalog.transition_failed",
    ),
    (
        DiagnosticCode::ReleaseTransitionFailed,
        "release_transition_failed",
        "release.transition_failed",
    ),
];

/// The table above names every variant: a new variant fails this exhaustive match.
fn listed(code: DiagnosticCode) -> bool {
    match code {
        DiagnosticCode::ProviderStartupFailed
        | DiagnosticCode::RuntimeDiagnostic
        | DiagnosticCode::RuntimeOwnerConflict
        | DiagnosticCode::RuntimeProtocolInvalid
        | DiagnosticCode::LeaseBusy
        | DiagnosticCode::LeaseCooldown
        | DiagnosticCode::LeaseExpired
        | DiagnosticCode::LeaseFencingDenied
        | DiagnosticCode::LeaseQueueCancelled
        | DiagnosticCode::LeaseQueueExpired
        | DiagnosticCode::LeaseQueueDisconnected
        | DiagnosticCode::BackendOpenFailed
        | DiagnosticCode::BackendOperationFailed
        | DiagnosticCode::CaptureFailed
        | DiagnosticCode::ArtifactWriteFailed
        | DiagnosticCode::ArtifactVerifyFailed
        | DiagnosticCode::ArtifactExportFailed
        | DiagnosticCode::PinnedFrameMissing
        | DiagnosticCode::RecognitionFailed
        | DiagnosticCode::InputFailed
        | DiagnosticCode::ApplicationFailed
        | DiagnosticCode::CommandRejected
        | DiagnosticCode::PolicyRejected
        | DiagnosticCode::CatalogTransitionFailed
        | DiagnosticCode::ReleaseTransitionFailed => true,
    }
}

/// #378 A2a T2 (brief-a2a.md; model-378 §2.2 :90, coordinator answer D4): every
/// `DiagnosticCode` variant serialises to its registered spelling, which is an active contract
/// code of the embedded catalog, and decodes back; its dotted spelling from before A2a is
/// refused (no alias).
#[test]
fn gate_diagnostic_codes_travel_as_registered_contract_codes_without_an_alias() {
    let catalog = actingcommand_contract::outcome::catalog_json().expect("the catalog parses");
    for (code, spelling, before) in DIAGNOSTIC_CODES {
        assert!(listed(code));
        assert_eq!(
            serde_json::to_value(code).expect("serialise the code"),
            json!(spelling),
            "{code:?}"
        );
        let entry = &catalog["codes"][spelling];
        assert_eq!(entry["owner"], "contract", "{spelling}");
        assert_eq!(entry["status"], "active", "{spelling}");
        assert_eq!(
            serde_json::from_value::<DiagnosticCode>(json!(spelling)).ok(),
            Some(code),
            "{spelling}"
        );
        if before != spelling {
            assert!(
                serde_json::from_value::<DiagnosticCode>(json!(before)).is_err(),
                "{before} is still accepted"
            );
        }
    }
}

const REASON: &str = "operator_hold";

/// `document` with the pause reason under `member`.
fn with_reason(mut document: Value, member: &str, reason: &str) -> Value {
    document
        .as_object_mut()
        .expect("a JSON object")
        .insert(member.to_owned(), json!(reason));
    document
}

/// Decodes and validates one pause document and encodes it again; `None` when either step
/// refuses it.
type Accepted = fn(&Value) -> Option<Value>;

/// The decoded and validated value, encoded again; `None` when either step refuses it.
fn accepted_pause_state(document: &Value) -> Option<Value> {
    let state = serde_json::from_value::<SchedulingPauseState>(document.clone()).ok()?;
    state.validate().ok()?;
    serde_json::to_value(&state).ok()
}

fn accepted_instance_pause_state(document: &Value) -> Option<Value> {
    let state = serde_json::from_value::<InstancePauseState>(document.clone()).ok()?;
    state.validate().ok()?;
    serde_json::to_value(&state).ok()
}

fn accepted_pause_scheduling(document: &Value) -> Option<Value> {
    let operation = serde_json::from_value::<RuntimeOperation>(document.clone()).ok()?;
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        operation.clone(),
    )
    .ok()?;
    serde_json::to_value(&operation).ok()
}

/// #378 A2a T3 (brief-a2a.md; model-378 §16 A-9 :633, coordinator answer D8): the global and
/// the instance pause state and the `PauseScheduling` request carry their reason under
/// `pause_reason`, round-trip, and refuse the old member `reason_code`; the reason still follows
/// its 1..=64 byte `[a-z0-9_.-]` rule.
#[test]
fn gate_scheduling_pause_reason_travels_as_pause_reason() {
    let too_long = "a".repeat(65);
    let shapes: [(&str, Value, Accepted); 3] = [
        (
            "global pause state",
            json!({"revision": 1, "since_unix_ms": 1}),
            accepted_pause_state,
        ),
        (
            "instance pause state",
            json!({"revision": 1, "since_unix_ms": 1, "stage": "paused"}),
            accepted_instance_pause_state,
        ),
        (
            "pause scheduling request",
            json!({
                "operation": "pause_scheduling",
                "scope": {"kind": "global"},
                "drain_timeout_ms": 1_000
            }),
            accepted_pause_scheduling,
        ),
    ];
    for (shape, base, accepted) in shapes {
        let current = with_reason(base.clone(), "pause_reason", REASON);
        assert_eq!(accepted(&current), Some(current.clone()), "{shape}");
        let old = with_reason(base.clone(), "reason_code", REASON);
        assert_eq!(accepted(&old), None, "{shape}");
        let empty = with_reason(base.clone(), "pause_reason", "");
        assert_eq!(accepted(&empty), None, "{shape}");
        let long = with_reason(base, "pause_reason", &too_long);
        assert_eq!(accepted(&long), None, "{shape}");
    }
}
