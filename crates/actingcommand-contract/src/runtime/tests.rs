// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{
    AuditInput, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity,
    EventType, IdentifierIssuer, LeasePayloadDraft, OriginModule, RuntimePayloadDraft,
    SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
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
fn runtime_info_accepts_only_live_loopback_shape() {
    let epoch = *issuer().mint_owner_epoch().expect("epoch").transport();
    let info = RuntimeInfo::new(1, "127.0.0.1", 48761, epoch, 1).expect("runtime info");
    assert!(info.socket_addr().expect("socket").ip().is_loopback());
    assert_eq!(
        RuntimeInfo::new(1, "0.0.0.0", 48761, epoch, 1)
            .expect_err("non-loopback host")
            .code(),
        "invalid_runtime_info"
    );
}

#[test]
fn readonly_capability_serializes_only_instance_authority() {
    let instance_id = *issuer().mint_instance_id().expect("instance").transport();
    let capability = ReadOnlyCaptureCapability::new(instance_id);
    let value = serde_json::to_value(&capability).expect("capability json");
    assert_eq!(value.as_object().expect("object").len(), 1);
    assert_eq!(capability.instance_id(), instance_id);
}

#[test]
fn query_result_remains_typed_without_generic_value_payload() {
    let result = RuntimeResult::Events { events: Vec::new() };
    let value = serde_json::to_value(result).expect("result json");
    assert_eq!(value["kind"], "events");
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
