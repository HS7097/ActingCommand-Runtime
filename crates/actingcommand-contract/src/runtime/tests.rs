// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactProducer,
    ArtifactRedactionState, ArtifactStoreIssuer, AuditInput, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventType, IdentifierIssuer, LeasePayloadDraft,
    OriginModule, RetentionClass, RuntimeMonitorState, RuntimePayloadDraft, SanitizationError,
    SecretField, SecretFingerprinter, Sha256Fingerprint,
};

struct RejectSecrets;

impl SecretFingerprinter for RejectSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        Err(SanitizationError::fingerprinter_failure())
    }
}

fn issuer() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn request(operation: RuntimeOperation) -> RuntimeRequest {
    let ids = issuer();
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        operation,
    )
    .expect("runtime request")
}

fn token() -> LeaseToken {
    let ids = issuer();
    LeaseToken::new(
        *ids.mint_owner_epoch().expect("epoch").transport(),
        *ids.mint_lease_id().expect("lease").transport(),
        *ids.mint_instance_id().expect("instance").transport(),
        *ids.mint_holder_id().expect("holder").transport(),
        100,
    )
    .expect("lease token")
}

#[test]
fn runtime_request_round_trips_and_builds_verified_event_links() {
    let ids = issuer();
    let request_id = ids.mint_request_id().expect("request id");
    let correlation_id = ids.mint_correlation_id().expect("correlation id");
    let holder_id = ids.mint_holder_id().expect("holder id");
    let request = RuntimeRequest::new(
        request_id,
        correlation_id,
        None,
        EventActor::Lab,
        EventSource::Lab,
        42,
        RuntimeOperation::acquire_lease("azur.jp", holder_id),
    )
    .expect("request");
    let encoded = serde_json::to_string(&request).expect("serialize request");
    let decoded: RuntimeRequest = serde_json::from_str(&encoded).expect("deserialize request");
    let instance_id = *ids.mint_instance_id().expect("instance id").transport();
    let lease_id = *ids.mint_lease_id().expect("lease id").transport();
    let action_id = *ids.mint_action_id().expect("action id").transport();
    let links = decoded.validate().expect("validate request").event_links(
        Some(instance_id),
        Some(lease_id),
        Some(action_id),
    );
    assert_eq!(links.request_id(), Some(&decoded.request_id()));
    assert_eq!(links.correlation_id(), Some(&decoded.correlation_id()));
    assert_eq!(links.instance_id(), Some(&instance_id));
    assert_eq!(links.lease_id(), Some(&lease_id));
    assert_eq!(links.action_id(), Some(&action_id));
}

#[test]
fn runtime_request_rejects_unknown_fields_schema_origin_and_alias() {
    let valid = serde_json::to_value(request(RuntimeOperation::Health)).expect("request json");
    let mut unknown = valid.clone();
    unknown
        .as_object_mut()
        .expect("object")
        .insert("unexpected".to_string(), serde_json::json!(true));
    assert!(serde_json::from_value::<RuntimeRequest>(unknown).is_err());

    let mut wrong_schema = valid.clone();
    wrong_schema["schema_version"] = serde_json::json!("actingcommand.runtime.request.v0");
    let wrong_schema: RuntimeRequest = serde_json::from_value(wrong_schema).expect("wire shape");
    assert_eq!(
        wrong_schema
            .validate()
            .expect_err("schema must fail")
            .code(),
        "unsupported_request_schema"
    );

    let mut wrong_origin = valid;
    wrong_origin["actor"] = serde_json::json!("system");
    wrong_origin["source"] = serde_json::json!("system");
    let wrong_origin: RuntimeRequest = serde_json::from_value(wrong_origin).expect("wire shape");
    assert_eq!(
        wrong_origin
            .validate()
            .expect_err("origin must fail")
            .code(),
        "invalid_client_origin"
    );

    let ids = issuer();
    let bad_alias = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::acquire_lease("", ids.mint_holder_id().expect("holder")),
    );
    assert_eq!(
        bad_alias.expect_err("alias must fail").code(),
        "invalid_instance_alias"
    );
}

#[test]
fn runtime_request_debug_redacts_alias_key_and_text() {
    let alias = "127.0.0.1:16384-private";
    let secret_text = "private-input-text";
    let secret_key = "private-key";
    let ids = issuer();
    let acquire = request(RuntimeOperation::acquire_lease(
        alias,
        ids.mint_holder_id().expect("holder"),
    ));
    let text = request(RuntimeOperation::Input {
        token: token(),
        action: InputAction::Text {
            text: secret_text.to_string(),
        },
    });
    let key = request(RuntimeOperation::Input {
        token: token(),
        action: InputAction::Key {
            key: secret_key.to_string(),
        },
    });
    let debug = format!("{acquire:?}{text:?}{key:?}");
    assert!(!debug.contains(alias));
    assert!(!debug.contains(secret_text));
    assert!(!debug.contains(secret_key));
}

#[test]
fn application_lifecycle_request_is_instance_scoped_and_carries_no_application_identity() {
    let ids = issuer();
    let operation = RuntimeOperation::application_lifecycle(
        "neutral.instance",
        ids.mint_holder_id().expect("holder"),
        ApplicationLifecycleAction::Restart,
    );
    operation.validate().expect("valid application lifecycle");
    let encoded = serde_json::to_value(&operation).expect("serialize operation");

    assert_eq!(encoded["operation"], "application_lifecycle");
    assert_eq!(encoded["instance_alias"], "neutral.instance");
    assert_eq!(encoded["action"], "restart");
    assert!(encoded.get("application_id").is_none());
    assert!(encoded.get("package").is_none());
    assert!(!format!("{operation:?}").contains("neutral.instance"));
}

#[test]
fn package_debug_contract_is_lab_only_bounded_and_redacted() {
    let private_path = r"C:\private\resource-package.zip";
    let expected = "a".repeat(64);
    let debug_request = PackageDebugRequest::new(private_path, &expected).expect("debug request");
    let debug = format!("{debug_request:?}");
    assert!(!debug.contains(private_path));
    assert!(!debug.contains(&expected));

    let ids = issuer();
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        RuntimeOperation::DebugPackage {
            request: debug_request,
        },
    )
    .expect("Lab package debug request");
    assert!(request.validate().is_ok());

    let ids = issuer();
    let wrong_origin = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::DebugPackage {
            request: PackageDebugRequest::new(private_path, &expected).expect("debug request"),
        },
    )
    .expect_err("non-Lab package debug must fail");
    assert_eq!(wrong_origin.code(), "invalid_runtime_debug_origin");

    assert_eq!(
        PackageDebugRequest::new(private_path, "A".repeat(64))
            .expect_err("uppercase hash")
            .code(),
        "invalid_debug_package_hash"
    );
    assert_eq!(
        PackageDebugRequest::new("", &expected)
            .expect_err("empty path")
            .code(),
        "invalid_debug_package_path"
    );
}

#[test]
fn runtime_debug_events_are_strict_and_require_lab_origin() {
    let mut invalid_progress =
        serde_json::to_value(RuntimeDebugEvent::progress(RuntimeDebugOperation::LabRun))
            .expect("debug event JSON");
    invalid_progress["operation"] = serde_json::json!("observe");
    let invalid_progress: RuntimeDebugEvent =
        serde_json::from_value(invalid_progress).expect("debug event wire");
    assert_eq!(
        invalid_progress
            .validate()
            .expect_err("only Lab run can report progress")
            .code(),
        "invalid_runtime_debug_event"
    );

    let mut invalid_effect =
        serde_json::to_value(RuntimeDebugEvent::requested(RuntimeDebugOperation::Observe))
            .expect("debug event JSON");
    invalid_effect["effect_disposition"] = serde_json::json!("performed");
    let invalid_effect: RuntimeDebugEvent =
        serde_json::from_value(invalid_effect).expect("debug event wire");
    assert_eq!(
        invalid_effect
            .validate()
            .expect_err("request cannot claim an effect")
            .code(),
        "invalid_runtime_debug_event"
    );

    let ids = issuer();
    let wrong_origin = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::RecordDebugEvent {
            event: RuntimeDebugEvent::completed(
                RuntimeDebugOperation::Observe,
                EffectDisposition::NotPerformed,
            ),
        },
    )
    .expect_err("non-Lab debug event must fail");
    assert_eq!(wrong_origin.code(), "invalid_runtime_debug_origin");
}

#[test]
fn package_debug_summary_round_trips_as_a_closed_result() {
    let summary = PackageDebugSummary::new(
        "task",
        "b".repeat(64),
        PackageDebugLayout::Lab,
        3,
        128,
        1,
        true,
        true,
        false,
    )
    .expect("summary");
    let result = RuntimeResult::PackageDebugCompleted {
        summary: summary.clone(),
    };
    let encoded = serde_json::to_string(&result).expect("serialize");
    let decoded: RuntimeResult = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, result);
    assert_eq!(summary.task_id(), "task");
    assert_eq!(summary.entry_count(), 3);
}

#[test]
fn evidence_export_request_is_lab_only_bounded_redacted_and_closed() {
    let private_path = r"C:\private\runtime-evidence.zip";
    let export = RuntimeEvidenceExportRequest::new(private_path, TaskOutcome::Success)
        .expect("evidence request");
    let debug = format!("{export:?}");
    assert!(!debug.contains(private_path));

    let ids = issuer();
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        RuntimeOperation::ExportEvidence {
            request: export.clone(),
        },
    )
    .expect("Lab evidence export request");
    let encoded = serde_json::to_string(&request).expect("serialize request");
    let decoded = serde_json::from_str::<RuntimeRequest>(&encoded).expect("deserialize request");
    assert_eq!(decoded, request);

    let ids = issuer();
    let wrong_origin = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::ExportEvidence { request: export },
    )
    .expect_err("non-Lab evidence export must fail");
    assert_eq!(wrong_origin.code(), "invalid_runtime_debug_origin");
    assert_eq!(
        RuntimeEvidenceExportRequest::new("", TaskOutcome::Success)
            .expect_err("empty path")
            .code(),
        "invalid_evidence_output_path"
    );
}

#[test]
fn every_input_variant_validates_bounds_and_maps_to_a_schema_action() {
    let actions = [
        InputAction::Tap { x: 1, y: 2 },
        InputAction::LongTap {
            x: 1,
            y: 2,
            duration_ms: 3,
        },
        InputAction::Swipe {
            x1: 1,
            y1: 2,
            x2: 3,
            y2: 4,
            duration_ms: 5,
        },
        InputAction::Key {
            key: "4".to_string(),
        },
        InputAction::Text {
            text: "hello".to_string(),
        },
        InputAction::Reset,
    ];
    for action in actions {
        action.validate().expect("valid action");
        assert!(action.event_action().as_str().starts_with("input."));
    }
    assert_eq!(
        InputAction::Tap { x: -1, y: 0 }
            .validate()
            .expect_err("negative coordinate")
            .code(),
        "invalid_input_coordinate"
    );
    assert_eq!(
        InputAction::LongTap {
            x: 0,
            y: 0,
            duration_ms: 0,
        }
        .validate()
        .expect_err("zero duration")
        .code(),
        "invalid_input_duration"
    );
    assert_eq!(
        InputAction::Text {
            text: "\0".to_string(),
        }
        .validate()
        .expect_err("nul text")
        .code(),
        "invalid_input_text"
    );
}

#[test]
fn lease_token_binds_all_fencing_fields_and_rejects_zero_expiry() {
    let token = token();
    let encoded = serde_json::to_string(&token).expect("serialize token");
    let decoded: LeaseToken = serde_json::from_str(&encoded).expect("deserialize token");
    assert_eq!(decoded, token);
    assert!(encoded.contains("epoch_"));
    assert!(encoded.contains("lease_"));

    let mut invalid = serde_json::to_value(token).expect("token value");
    invalid["expires_at_monotonic_ms"] = serde_json::json!(0);
    let invalid: LeaseToken = serde_json::from_value(invalid).expect("wire token");
    assert_eq!(
        invalid.validate().expect_err("zero expiry").code(),
        "invalid_lease_expiry"
    );
}

#[test]
fn c3b_queue_policy_and_status_are_closed_bounded_and_strict() {
    let policy = LeaseQueuePolicy::new(LeasePriority::High, 1_000).expect("queue policy");
    assert_eq!(policy.priority(), LeasePriority::High);
    assert_eq!(policy.timeout_ms(), 1_000);
    assert_eq!(
        LeaseQueuePolicy::new(LeasePriority::Normal, 0)
            .expect_err("zero timeout")
            .code(),
        "invalid_lease_queue_timeout"
    );
    assert_eq!(
        LeaseQueuePolicy::new(LeasePriority::Normal, MAX_LEASE_QUEUE_TIMEOUT_MS + 1)
            .expect_err("unbounded timeout")
            .code(),
        "invalid_lease_queue_timeout"
    );

    let ids = issuer();
    let status = LeaseQueueStatus::new(
        *ids.mint_request_id().expect("request").transport(),
        *ids.mint_instance_id().expect("instance").transport(),
        LeasePriority::High,
        1,
        500,
        true,
    )
    .expect("queue status");
    let encoded = serde_json::to_string(&status).expect("status JSON");
    let decoded: LeaseQueueStatus = serde_json::from_str(&encoded).expect("strict status");
    assert_eq!(decoded, status);
    assert!(decoded.preempt_requested());

    let mut unknown = serde_json::to_value(&status).expect("status value");
    unknown["smuggled"] = serde_json::json!(true);
    assert!(serde_json::from_value::<LeaseQueueStatus>(unknown).is_err());
    assert_eq!(
        LeaseQueueStatus::new(
            status.request_id(),
            status.instance_id(),
            LeasePriority::Normal,
            0,
            500,
            false,
        )
        .expect_err("zero queue position")
        .code(),
        "invalid_lease_queue_status"
    );
    assert!(serde_json::from_str::<LeasePriority>("\"urgent\"").is_err());
}

#[test]
fn runtime_status_is_sorted_strict_and_state_aware() {
    let ids = issuer();
    let owner_epoch = *ids.mint_owner_epoch().expect("owner epoch").transport();
    let ak = RuntimeInstanceStatus::new(
        "ak.cn",
        *ids.mint_instance_id().expect("ak instance").transport(),
        true,
        1,
        false,
        true,
        true,
    )
    .expect("ak status");
    let ba = RuntimeInstanceStatus::new(
        "ba.jp",
        *ids.mint_instance_id().expect("ba instance").transport(),
        false,
        0,
        true,
        false,
        false,
    )
    .expect("ba status");
    let status = RuntimeControlPlaneStatus::new(owner_epoch, vec![ba, ak]).expect("status");

    assert_eq!(status.owner_epoch(), owner_epoch);
    assert_eq!(status.instances()[0].instance_alias(), "ak.cn");
    assert!(status.instances()[0].lease_active());
    assert_eq!(status.instances()[0].queued_request_count(), 1);
    assert!(status.instances()[0].destructive_step_active());
    assert!(status.instances()[0].preempt_requested());
    assert!(status.instances()[1].takeover_cooldown_active());

    let encoded = serde_json::to_value(&status).expect("status JSON");
    let decoded: RuntimeControlPlaneStatus =
        serde_json::from_value(encoded.clone()).expect("status decode");
    decoded.validate().expect("status validation");
    let request = request(RuntimeOperation::Status);
    RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::Status {
            status: status.clone(),
        },
    )
    .expect("status receipt")
    .validate()
    .expect("status receipt validation");

    let mut unknown = encoded;
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RuntimeControlPlaneStatus>(unknown).is_err());
    assert_eq!(
        RuntimeInstanceStatus::new(
            "ak.cn",
            status.instances()[0].instance_id(),
            false,
            0,
            false,
            true,
            false,
        )
        .expect_err("destructive state without lease")
        .code(),
        "invalid_runtime_instance_status"
    );
}

#[test]
fn runtime_monitor_operations_and_receipts_are_strict() {
    let ids = issuer();
    let owner_epoch = *ids.mint_owner_epoch().expect("owner epoch").transport();
    let policy = RuntimeMonitorPolicy::new(1_000, "home", true).expect("policy");
    let configure = request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
        policy: policy.clone(),
    });
    configure.validate().expect("configure request");
    request(RuntimeOperation::ClearMonitor {
        instance_alias: "ak.cn".to_string(),
    })
    .validate()
    .expect("clear request");

    let configured = RuntimeMonitorInstanceStatus::configured(
        "ak.cn",
        policy,
        RuntimeMonitorState::scheduled(10).expect("state"),
    )
    .expect("configured status");
    RuntimeReceipt::success(
        &configure,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::MonitorConfigured {
            status: configured.clone(),
        },
    )
    .expect("configured receipt")
    .validate()
    .expect("configured receipt validation");

    let status_request = request(RuntimeOperation::MonitorStatus);
    let registry =
        RuntimeMonitorRegistryStatus::new(owner_epoch, vec![configured]).expect("monitor registry");
    RuntimeReceipt::success(
        &status_request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::MonitorStatus { status: registry },
    )
    .expect("monitor status receipt")
    .validate()
    .expect("monitor status receipt validation");

    let mut encoded = serde_json::to_value(configure.operation()).expect("operation JSON");
    encoded["policy"]["unknown"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RuntimeOperation>(encoded).is_err());
}

#[test]
fn receipt_requires_exactly_one_typed_outcome() {
    let request = request(RuntimeOperation::Health);
    let epoch = *issuer().mint_owner_epoch().expect("epoch").transport();
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::Health { owner_epoch: epoch },
    )
    .expect("success receipt");
    let encoded = serde_json::to_string(&receipt).expect("serialize receipt");
    let decoded: RuntimeReceipt = serde_json::from_str(&encoded).expect("deserialize receipt");
    decoded.validate().expect("validate receipt");

    let mut invalid = serde_json::to_value(receipt).expect("receipt value");
    invalid["state"] = serde_json::json!("failed");
    let invalid: RuntimeReceipt = serde_json::from_value(invalid).expect("wire receipt");
    assert_eq!(
        invalid.validate().expect_err("outcome mismatch").code(),
        "invalid_receipt_outcome"
    );
}

#[test]
fn queued_receipt_is_successful_but_distinct_from_granted_authority() {
    let request = request(RuntimeOperation::Health);
    let ids = issuer();
    let status = LeaseQueueStatus::new(
        request.request_id(),
        *ids.mint_instance_id().expect("instance").transport(),
        LeasePriority::Normal,
        1,
        100,
        false,
    )
    .expect("queue status");
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Queued,
        None,
        RuntimeResult::LeaseQueued {
            status: status.clone(),
        },
    )
    .expect("queued receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Queued);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::LeaseQueued { status: actual }) if actual == &status
    ));

    let mut invalid = serde_json::to_value(&receipt).expect("receipt value");
    invalid["result"]["status"]["position"] = serde_json::json!(0);
    let invalid: RuntimeReceipt = serde_json::from_value(invalid).expect("wire receipt");
    assert_eq!(
        invalid.validate().expect_err("invalid queue status").code(),
        "invalid_lease_queue_status"
    );
}

#[test]
fn c3b_queue_operations_and_cancelled_receipt_are_strict_typed_contracts() {
    let ids = issuer();
    let queued_request_id = *ids.mint_request_id().expect("queued request").transport();
    let operations = [
        RuntimeOperation::queue_lease(
            "ak.cn",
            ids.mint_holder_id().expect("holder"),
            LeaseQueuePolicy::new(LeasePriority::High, 1_000).expect("policy"),
        ),
        RuntimeOperation::PollQueuedLease { queued_request_id },
        RuntimeOperation::CancelQueuedLease { queued_request_id },
    ];
    for operation in operations {
        operation.validate().expect("operation");
        let encoded = serde_json::to_string(&operation).expect("operation JSON");
        let decoded: RuntimeOperation = serde_json::from_str(&encoded).expect("operation decode");
        assert_eq!(decoded, operation);
    }

    let request = request(RuntimeOperation::CancelQueuedLease { queued_request_id });
    let instance_id = *ids.mint_instance_id().expect("instance").transport();
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Cancelled,
        None,
        RuntimeResult::LeaseQueueCancelled {
            request_id: queued_request_id,
            instance_id,
        },
    )
    .expect("cancelled receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Cancelled);
    receipt.validate().expect("cancelled receipt validation");
}

#[test]
fn runtime_info_accepts_only_live_loopback_shape() {
    let epoch = *issuer().mint_owner_epoch().expect("epoch").transport();
    let info = RuntimeInfo::new(1, "127.0.0.1", 48761, epoch, 1).expect("runtime info");
    assert!(info.socket_addr().expect("socket").ip().is_loopback());
    assert_eq!(info.started_at_unix_ms(), 1);
    assert_eq!(
        RuntimeInfo::new(1, "0.0.0.0", 48761, epoch, 1)
            .expect_err("non-loopback host")
            .code(),
        "invalid_runtime_info"
    );
}

#[test]
fn readonly_capability_is_issuer_owned_and_binds_observation_context() {
    let ids = issuer();
    let owner_epoch = *ids.mint_owner_epoch().expect("epoch").transport();
    let instance_id = *ids.mint_instance_id().expect("instance").transport();
    let issued = ids
        .issue_readonly_capture_capability(owner_epoch, instance_id)
        .expect("read-only capability");
    let capability = issued.transport();
    let value = serde_json::to_value(capability).expect("capability json");

    assert_eq!(value.as_object().expect("object").len(), 4);
    assert_eq!(capability.instance_id(), instance_id);
    assert_eq!(
        value["owner_epoch"],
        serde_json::to_value(owner_epoch).expect("owner epoch JSON")
    );
    assert!(
        value["frame_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("frame_"))
    );
    assert!(
        value["recognition_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("recognition_"))
    );

    let request = request(RuntimeOperation::ObserveReadonly {
        instance_alias: "ak.cn".to_string(),
    });
    let links = issued.event_links(&request.validate().expect("validated request"));
    assert_eq!(links.instance_id(), Some(&instance_id));
    assert_eq!(
        links
            .frame_id()
            .map(|value| serde_json::to_value(value).expect("frame JSON")),
        Some(value["frame_id"].clone())
    );
    assert_eq!(links.recognition_id(), Some(&capability.recognition_id()));
}

#[test]
fn readonly_observation_is_closed_typed_and_nonzero() {
    let artifact = observation_artifact();
    let observation = ReadonlyObservation::new(
        1280,
        720,
        RecognitionVerdict::FrameDecoded,
        RuntimeCaptureBackend::NemuIpc,
        artifact.clone(),
    )
    .expect("observation");
    assert_eq!(observation.width(), 1280);
    assert_eq!(observation.height(), 720);
    assert_eq!(observation.verdict(), RecognitionVerdict::FrameDecoded);
    assert_eq!(
        observation.capture_backend(),
        RuntimeCaptureBackend::NemuIpc
    );
    assert_eq!(observation.artifact(), &artifact);
    assert!(!format!("{observation:?}").contains(artifact.object_key.as_deref().unwrap()));

    assert_eq!(
        ReadonlyObservation::new(
            0,
            720,
            RecognitionVerdict::FrameDecoded,
            RuntimeCaptureBackend::NemuIpc,
            artifact.clone(),
        )
        .expect_err("zero width")
        .code(),
        "invalid_observation_dimensions"
    );

    let mut missing_object = artifact.clone();
    missing_object.object_key = None;
    assert_eq!(
        ReadonlyObservation::new(
            1280,
            720,
            RecognitionVerdict::FrameDecoded,
            RuntimeCaptureBackend::NemuIpc,
            missing_object,
        )
        .expect_err("missing artifact object key")
        .code(),
        "invalid_observation_artifact"
    );

    let mut value = serde_json::to_value(&observation).expect("observation json");
    value["smuggled"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ReadonlyObservation>(value).is_err());
    assert!(serde_json::from_str::<RecognitionVerdict>("\"runtime-value\"").is_err());

    let captured = ReadonlyFrame::new(1280, 720).expect("captured frame");
    assert_eq!(captured.width(), 1280);
    assert_eq!(captured.height(), 720);
    assert_eq!(
        ReadonlyFrame::new(1280, 0)
            .expect_err("zero frame height")
            .code(),
        "invalid_frame_dimensions"
    );

    ReadonlyObservationOutcome::Failed {
        stage: ReadonlyObservationStage::Capture,
        captured_frame: None,
    }
    .validate()
    .expect("capture failure has no frame");
    ReadonlyObservationOutcome::Failed {
        stage: ReadonlyObservationStage::Recognition,
        captured_frame: Some(captured),
    }
    .validate()
    .expect("recognition failure preserves captured frame");
    assert_eq!(
        ReadonlyObservationOutcome::Failed {
            stage: ReadonlyObservationStage::Capture,
            captured_frame: Some(captured),
        }
        .validate()
        .expect_err("capture failure cannot claim a frame")
        .code(),
        "invalid_observation_failure_context"
    );
    assert_eq!(
        ReadonlyObservationOutcome::Failed {
            stage: ReadonlyObservationStage::Recognition,
            captured_frame: None,
        }
        .validate()
        .expect_err("recognition failure requires captured frame")
        .code(),
        "invalid_observation_failure_context"
    );
}

fn observation_artifact() -> ProjectedArtifactReference {
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let frame = identifiers.mint_frame_id().expect("frame");
    ArtifactStoreIssuer::new()
        .expect("artifact issuer")
        .issue(
            ArtifactKind::CaptureFrame,
            ArtifactLinksDraft::default().with_frame_id(frame),
            b"observation-png",
            1_752_147_200_000,
            ArtifactIssuePolicy::new(
                ArtifactProducer::CaptureStore,
                RetentionClass::DebugFull,
                ArtifactRedactionState::NotRequired,
            ),
        )
        .expect("artifact")
        .reference()
        .project(true)
}

fn readonly_observation() -> ReadonlyObservation {
    ReadonlyObservation::new(
        1280,
        720,
        RecognitionVerdict::FrameDecoded,
        RuntimeCaptureBackend::NemuIpc,
        observation_artifact(),
    )
    .expect("observation")
}

#[test]
fn capture_sequence_contract_is_bounded_exact_and_strict() {
    let spec = CaptureSequenceSpec::new(60, 1_000).expect("maximum bounded sequence");
    assert_eq!(spec.frame_count(), 60);
    assert_eq!(spec.interval_ms(), 1_000);
    assert_eq!(spec.planned_wait_ms().expect("planned wait"), 59_000);

    for (frame_count, interval_ms) in [(0, 0), (61, 0), (2, 5_001), (60, 5_000)] {
        assert!(
            CaptureSequenceSpec::new(frame_count, interval_ms).is_err(),
            "invalid sequence bounds must fail: {frame_count} frames at {interval_ms} ms"
        );
    }

    let pair_spec = CaptureSequenceSpec::new(2, 25).expect("pair spec");
    let first = readonly_observation();
    let second = readonly_observation();
    let sequence =
        CaptureSequence::new(pair_spec, vec![first.clone(), second]).expect("capture sequence");
    assert_eq!(sequence.spec(), pair_spec);
    assert_eq!(sequence.observations().len(), 2);
    assert_ne!(
        sequence.observations()[0].artifact().artifact_id,
        sequence.observations()[1].artifact().artifact_id
    );

    assert_eq!(
        CaptureSequence::new(pair_spec, vec![first.clone()])
            .expect_err("exact observation count")
            .code(),
        "invalid_capture_sequence_observation_count"
    );
    assert_eq!(
        CaptureSequence::new(pair_spec, vec![first.clone(), first])
            .expect_err("duplicate artifact identity")
            .code(),
        "duplicate_capture_sequence_artifact_identity"
    );

    let operation = RuntimeOperation::CaptureSequence {
        instance_alias: "ak.cn".to_string(),
        spec: pair_spec,
    };
    request(operation.clone())
        .validate()
        .expect("capture sequence request");
    let encoded = serde_json::to_value(operation).expect("capture sequence operation JSON");
    assert_eq!(encoded["operation"], "capture_sequence");
    assert!(encoded.get("token").is_none());
    assert!(encoded.get("action").is_none());
    let mut unknown = encoded;
    unknown["spec"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<RuntimeOperation>(unknown).is_err());

    RuntimeReceipt::success(
        &request(RuntimeOperation::CaptureSequence {
            instance_alias: "ak.cn".to_string(),
            spec: pair_spec,
        }),
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::CaptureSequenceCompleted { sequence },
    )
    .expect("capture sequence receipt")
    .validate()
    .expect("capture sequence receipt validation");
}

#[test]
fn c4_operations_round_trip_without_generic_payloads() {
    let ids = issuer();
    let operations = [
        RuntimeOperation::ObserveReadonly {
            instance_alias: "ak.cn".to_string(),
        },
        RuntimeOperation::SafeReset {
            instance_alias: "ak.cn".to_string(),
            holder_id: *ids.mint_holder_id().expect("holder").transport(),
        },
    ];

    for operation in operations {
        let request = request(operation);
        let encoded = serde_json::to_string(&request).expect("request JSON");
        let decoded = serde_json::from_str::<RuntimeRequest>(&encoded).expect("request round trip");
        decoded.validate().expect("valid C4 request");
        assert!(!encoded.contains("serde_json::Value"));
        assert!(!encoded.contains("capability"));
    }

    assert!(
        serde_json::from_str::<RuntimeOperation>(
            r#"{"operation":"begin_readonly_observation","instance_alias":"ak.cn"}"#,
        )
        .is_err()
    );
}

#[test]
fn query_result_remains_typed_without_generic_value_payload() {
    let result = RuntimeResult::Events { events: Vec::new() };
    let value = serde_json::to_value(result).expect("result json");
    assert_eq!(value["kind"], "events");
}

#[test]
fn runtime_subscription_contract_is_bounded_and_strict() {
    let request = RuntimeSubscriptionRequest::new(
        EventQuery::default(),
        ProjectionProfile::Lab,
        SubscriptionCursor { after_sequence: 7 },
        1_000,
        64,
    )
    .expect("subscription request");
    let operation = RuntimeOperation::SubscribeEvents {
        request: request.clone(),
    };
    let encoded = serde_json::to_string(&operation).expect("subscription request JSON");
    let decoded: RuntimeOperation = serde_json::from_str(&encoded).expect("subscription request");
    decoded.validate().expect("valid subscription request");
    assert_eq!(request.cursor().after_sequence, 7);
    assert_eq!(request.wait_ms(), 1_000);
    assert_eq!(request.max_events(), 64);

    assert!(
        RuntimeSubscriptionRequest::new(
            EventQuery::default(),
            ProjectionProfile::Lab,
            SubscriptionCursor::default(),
            MAX_RUNTIME_SUBSCRIPTION_WAIT_MS + 1,
            1,
        )
        .is_err()
    );
    assert!(
        RuntimeSubscriptionRequest::new(
            EventQuery::default(),
            ProjectionProfile::Lab,
            SubscriptionCursor::default(),
            0,
            0,
        )
        .is_err()
    );
}

#[test]
fn runtime_subscription_timeout_batch_round_trips_without_fake_progress() {
    let batch = RuntimeEventBatch::new(Vec::new(), SubscriptionCursor { after_sequence: 9 }, true)
        .expect("timeout batch");
    let result = RuntimeResult::EventBatch {
        batch: batch.clone(),
    };
    let encoded = serde_json::to_string(&result).expect("event batch JSON");
    let decoded: RuntimeResult = serde_json::from_str(&encoded).expect("event batch");
    assert_eq!(decoded, result);
    assert!(batch.events().is_empty());
    assert!(batch.timed_out());
    assert!(RuntimeEventBatch::new(Vec::new(), SubscriptionCursor::default(), false).is_err());
}

#[test]
fn resource_authoring_event_is_strict_and_requires_lab_origin() {
    let event = ResourceAuthoringEvent::new(
        ResourceAuthoringPhase::PromoteIntent,
        "draft-a",
        "resource-root",
        "b".repeat(64),
        vec!["operations/task-a/task.json".to_string()],
        None,
    )
    .expect("authoring event");
    let operation = RuntimeOperation::RecordAuthoringEvent {
        event: event.clone(),
    };
    let ids = issuer();
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation.clone(),
    )
    .expect("Lab authoring request");
    let encoded = serde_json::to_string(&request).expect("authoring request JSON");
    let decoded: RuntimeRequest = serde_json::from_str(&encoded).expect("authoring request wire");
    decoded.validate().expect("valid authoring request");
    assert_eq!(event.phase(), ResourceAuthoringPhase::PromoteIntent);
    assert_eq!(event.changed_paths(), ["operations/task-a/task.json"]);
    assert!(!format!("{operation:?}").contains("operations/task-a"));

    let ids = issuer();
    let wrong_origin = match RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::RecordAuthoringEvent { event },
    ) {
        Ok(_) => panic!("CLI cannot record authoring events"),
        Err(error) => error,
    };
    assert_eq!(wrong_origin.code(), "invalid_resource_authoring_origin");
}

#[test]
fn resource_authoring_event_revalidates_deserialized_fields_and_receipt_phase() {
    let valid = ResourceAuthoringEvent::new(
        ResourceAuthoringPhase::Promoted,
        "draft-a",
        "resource-root",
        "b".repeat(64),
        vec!["operations/task-a/task.json".to_string()],
        None,
    )
    .expect("authoring event");
    let mut forged = serde_json::to_value(&valid).expect("authoring event JSON");
    forged["changed_paths"] = serde_json::json!(["../outside.json"]);
    let forged: ResourceAuthoringEvent =
        serde_json::from_value(forged).expect("transport shape remains valid");
    assert_eq!(
        forged.validate().expect_err("path escape must fail").code(),
        "invalid_resource_authoring_event"
    );

    let ids = issuer();
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        RuntimeOperation::RecordAuthoringEvent { event: valid },
    )
    .expect("authoring request");
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::AuthoringEventRecorded {
            phase: ResourceAuthoringPhase::Promoted,
        },
    )
    .expect("authoring receipt");
    let value = serde_json::to_value(receipt).expect("authoring receipt JSON");
    assert_eq!(value["result"]["kind"], "authoring_event_recorded");
    assert_eq!(value["result"]["phase"], "promoted");
}

#[test]
fn c3a_runtime_and_lease_renewal_events_are_typed() {
    let ids = issuer();
    let cases: [(EventPayloadDraft, EventType); 3] = [
        (
            RuntimePayloadDraft::started(crate::EventAction::RuntimeStart, AuditInput::new())
                .into(),
            EventType::RuntimeStarted,
        ),
        (
            RuntimePayloadDraft::takeover(crate::EventAction::RuntimeTakeover, AuditInput::new())
                .into(),
            EventType::RuntimeTakeover,
        ),
        (
            LeasePayloadDraft::renewed(
                crate::EventAction::LeaseRenew,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            EventType::LeaseRenewed,
        ),
    ];
    for (payload, expected) in cases {
        let draft = EventDraft::new(
            ids.mint_event_id().expect("event id"),
            1,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            payload,
        )
        .sanitize(&RejectSecrets)
        .expect("sanitize typed event");
        assert_eq!(draft.event_type(), expected);
        serde_json::to_string(&draft).expect("serialize typed event");
    }
}
