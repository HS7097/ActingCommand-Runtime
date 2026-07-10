use super::*;
use std::sync::Mutex;

struct SpyFingerprinter {
    seen: Mutex<Vec<(SecretField, String)>>,
}

impl SpyFingerprinter {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<(SecretField, String)> {
        self.seen.lock().expect("spy lock").clone()
    }
}

impl SecretFingerprinter for SpyFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        self.seen
            .lock()
            .expect("spy lock")
            .push((field, original.to_string()));
        Sha256Fingerprint::new(format!("sha256:{}", "a".repeat(64)), original)
    }
}

struct MaliciousEchoFingerprinter;

impl SecretFingerprinter for MaliciousEchoFingerprinter {
    fn fingerprint(
        &self,
        _field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        let candidate = if original.starts_with("sha256:") {
            original.to_string()
        } else {
            format!("sha256:{original}")
        };
        Sha256Fingerprint::new(candidate, "different-original")
    }
}

fn identifier_issuer() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn origin() -> EventOrigin {
    EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User)
}

fn links(issuer: &IdentifierIssuer) -> EventLinksDraft {
    EventLinksDraft::default()
        .with_instance_id(issuer.mint_instance_id().expect("instance id"))
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_causation_id(issuer.mint_causation_id().expect("causation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"))
        .with_lease_id(issuer.mint_lease_id().expect("lease id"))
        .with_frame_id(issuer.mint_frame_id().expect("frame id"))
        .with_action_id(issuer.mint_action_id().expect("action id"))
        .with_recognition_id(issuer.mint_recognition_id().expect("recognition id"))
}

fn artifact_links(issuer: &IdentifierIssuer) -> ArtifactLinksDraft {
    ArtifactLinksDraft::default()
        .with_run_id(issuer.mint_run_id().expect("run id"))
        .with_frame_id(issuer.mint_frame_id().expect("frame id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
}

fn artifact(bytes: &[u8]) -> StoreIssuedArtifact {
    let links_issuer = identifier_issuer();
    super::artifact::issue_pending_for_tests(
        ArtifactKind::CaptureFrame,
        artifact_links(&links_issuer),
        bytes,
        1_752_147_200_000,
    )
    .expect("store-issued artifact")
}

fn audit_all() -> AuditInput {
    AuditInput::new()
        .with_account("account-secret-c1@example.invalid")
        .with_authentication("authentication-secret-c1")
        .with_machine_path(r"C:\Users\Alice\private\runtime.json")
        .with_device_endpoint("127.0.0.1:16384")
}

fn all_payload_drafts(mut input: impl FnMut() -> AuditInput) -> Vec<EventPayloadDraft> {
    let action = EventAction::RuntimeAction;
    let diagnostic = DiagnosticCode::RuntimeDiagnostic;
    let effect = EffectDisposition::Performed;

    vec![
        CommandPayloadDraft::received(action, input()).into(),
        CommandPayloadDraft::validated(action, effect, input()).into(),
        CommandPayloadDraft::rejected(action, diagnostic, effect, input()).into(),
        SchedulerPayloadDraft::admitted(action, input()).into(),
        SchedulerPayloadDraft::queued(action, input()).into(),
        SchedulerPayloadDraft::denied(action, diagnostic, input()).into(),
        SchedulerPayloadDraft::preempted(action, diagnostic, input()).into(),
        LeasePayloadDraft::requested(action, input()).into(),
        LeasePayloadDraft::granted(action, effect, input()).into(),
        LeasePayloadDraft::transferred(action, effect, input()).into(),
        LeasePayloadDraft::released(action, effect, input()).into(),
        LeasePayloadDraft::expired(action, effect, input()).into(),
        LeasePayloadDraft::transition_intent(action, input()).into(),
        LeasePayloadDraft::transition_failed(action, diagnostic, effect, input()).into(),
        TaskPayloadDraft::requested(action, input()).into(),
        TaskPayloadDraft::started(action, input()).into(),
        TaskPayloadDraft::step_started(action, input()).into(),
        TaskPayloadDraft::step_finished(action, input()).into(),
        TaskPayloadDraft::completed(action, effect, input()).into(),
        TaskPayloadDraft::failed(action, diagnostic, effect, input()).into(),
        TaskPayloadDraft::cancelled(action, effect, input()).into(),
        TaskPayloadDraft::terminal_intent(action, input()).into(),
        TaskPayloadDraft::terminal_commit_failed(action, diagnostic, effect, input()).into(),
        InputPayloadDraft::intent(action, input()).into(),
        InputPayloadDraft::committed(action, effect, input()).into(),
        InputPayloadDraft::completed(action, input()).into(),
        InputPayloadDraft::failed(action, diagnostic, effect, input()).into(),
        ClientPayloadDraft::ui_action(action, input()).into(),
        ClientPayloadDraft::cli_command(action, input()).into(),
        ClientPayloadDraft::lab_request(action, input()).into(),
        LedgerPayloadDraft::recovered(RecoveryReason::StaleOwner, Some(1), 64, input()).into(),
    ]
}

fn sanitize_with(
    payload: EventPayloadDraft,
    index: u64,
    fingerprinter: &dyn SecretFingerprinter,
) -> SanitizedEventDraft {
    let issuer = identifier_issuer();
    EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000 + index,
        EventSeverity::Info,
        origin(),
        links(&issuer),
        payload,
    )
    .sanitize(fingerprinter)
    .expect("sanitize event")
}

fn sanitize(payload: EventPayloadDraft, index: u64) -> SanitizedEventDraft {
    sanitize_with(payload, index, &SpyFingerprinter::new())
}

#[test]
fn producer_cannot_select_redaction_policy() {
    let sanitized = sanitize(
        CommandPayloadDraft::received(EventAction::RuntimeStart, audit_all()).into(),
        1,
    );
    let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

    assert!(!json.contains("redaction_policy"));
    assert!(!json.contains("\"policy\""));
}

#[test]
fn all_runtime_secret_classes_follow_schema_owned_policy_independently() {
    let cases: [(&str, fn(&str) -> AuditInput); 4] = [
        ("account-secret-c1@example.invalid", |value| {
            AuditInput::new().with_account(value)
        }),
        ("authentication-secret-c1", |value| {
            AuditInput::new().with_authentication(value)
        }),
        (r"C:\Users\Alice\private\runtime.json", |value| {
            AuditInput::new().with_machine_path(value)
        }),
        ("127.0.0.1:16384", |value| {
            AuditInput::new().with_device_endpoint(value)
        }),
    ];

    for (secret, input) in cases {
        for (index, payload) in all_payload_drafts(|| input(secret)).into_iter().enumerate() {
            let spy = SpyFingerprinter::new();
            let sanitized = sanitize_with(payload, index as u64 + 1, &spy);
            let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");
            let debug = format!("{sanitized:?}");

            assert!(!json.contains(secret), "secret leaked: {secret}");
            assert!(!debug.contains(secret), "debug leaked: {secret}");
            if secret == "account-secret-c1@example.invalid" {
                assert_eq!(
                    spy.seen(),
                    vec![(SecretField::AccountIdentity, secret.to_string())]
                );
                assert!(json.contains(&format!("sha256:{}", "a".repeat(64))));
            } else {
                assert!(spy.seen().is_empty());
            }
        }
    }
}

#[test]
fn hash_shaped_original_cannot_survive_as_fingerprint() {
    for original in ["a".repeat(64), format!("sha256:{}", "a".repeat(64))] {
        let issuer = identifier_issuer();
        let draft = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            EventLinksDraft::default(),
            CommandPayloadDraft::received(
                EventAction::RuntimeStart,
                AuditInput::new().with_account(original.clone()),
            )
            .into(),
        );

        let error = draft
            .sanitize(&MaliciousEchoFingerprinter)
            .expect_err("hash echo must fail");
        assert_eq!(error.code(), "invalid_fingerprint");
        assert!(!error.to_string().contains(&original));
    }
}

#[test]
fn runtime_values_cannot_enter_origin_action_or_diagnostic_codes() {
    for runtime in [
        "token-secret-7d141b7b",
        "account-secret-valid-code",
        Box::leak(String::from("leaked-runtime-token").into_boxed_str()),
    ] {
        let json = serde_json::to_string(runtime).expect("runtime string");
        assert!(serde_json::from_str::<OriginModule>(&json).is_err());
        assert!(serde_json::from_str::<EventAction>(&json).is_err());
        assert!(serde_json::from_str::<DiagnosticCode>(&json).is_err());
        assert!(serde_json::from_str::<RecoveryReason>(&json).is_err());
    }

    assert_eq!(EventAction::RuntimeStart.to_string(), "runtime.start");
    assert_eq!(format!("{:?}", EventAction::RuntimeStart), "RuntimeStart");
    assert_eq!(OriginModule::Actingctl.to_string(), "actingctl");
    assert_eq!(
        DiagnosticCode::RuntimeDiagnostic.to_string(),
        "runtime.diagnostic"
    );
}

#[test]
fn canonical_transport_ids_cannot_be_promoted_to_producer_capabilities() {
    let canonical = format!("evt_{}", "ab".repeat(16));
    let transport: EventId =
        serde_json::from_str(&format!("\"{canonical}\"")).expect("transport event id");
    assert!(!format!("{transport:?}").contains(&canonical));

    let issuer = identifier_issuer();
    let issued = issuer.mint_event_id().expect("issued event id");
    let serialized = serde_json::to_string(issued.transport()).expect("serialize issued id");
    assert_ne!(serialized, format!("\"{canonical}\""));
    assert!(!format!("{issued:?}").contains(&canonical));
}

#[test]
fn every_action_and_diagnostic_slot_rejects_runtime_code_mutations() {
    for (index, payload) in all_payload_drafts(AuditInput::new).into_iter().enumerate() {
        let sanitized = sanitize(payload, index as u64 + 1);
        let value = serde_json::to_value(sanitized.payload()).expect("payload value");
        if value["payload"]["data"].get("action").is_some() {
            let mut action_mutation = value.clone();
            action_mutation["payload"]["data"]["action"] =
                serde_json::json!("token-secret-valid-code");
            assert!(serde_json::from_value::<EventPayload>(action_mutation).is_err());
        }
        if value["payload"]["data"].get("diagnostic_code").is_some() {
            let mut diagnostic_mutation = value;
            diagnostic_mutation["payload"]["data"]["diagnostic_code"] =
                serde_json::json!("account-secret-valid-code");
            assert!(serde_json::from_value::<EventPayload>(diagnostic_mutation).is_err());
        }
    }
}

#[test]
fn artifact_store_owns_complete_v3_metadata_and_event_sensitivity() {
    let issued = artifact(b"capture bytes");
    let reference = issued.reference();

    assert_eq!(reference.kind(), ArtifactKind::CaptureFrame);
    assert!(reference.run_id().is_some());
    assert!(reference.frame_id().is_some());
    assert!(reference.correlation_id().is_some());
    assert!(reference.object_key().starts_with("artifacts/"));
    assert_eq!(reference.media_type(), ArtifactMediaType::ImagePng);
    assert_eq!(reference.byte_count(), 13);
    assert_eq!(reference.created_at_unix_ms(), 1_752_147_200_000);
    assert_eq!(reference.producer(), ArtifactProducer::CaptureStore);
    assert_eq!(reference.retention_class(), RetentionClass::Adaptive);
    assert_eq!(reference.redaction_state(), ArtifactRedactionState::Pending);
    assert_eq!(reference.sensitivity(), Sensitivity::Secret);

    let payload = CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new());
    let issuer = identifier_issuer();
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_752_147_200_000,
        EventSeverity::Info,
        origin(),
        EventLinksDraft::default(),
        payload.into(),
    )
    .with_artifacts(vec![issued])
    .sanitize(&SpyFingerprinter::new())
    .expect("sanitize artifact event");

    assert_eq!(sanitized.sensitivity(), Sensitivity::Secret);
}

#[test]
fn artifact_secret_classes_cannot_survive_any_metadata_or_diagnostic_surface() {
    for secret in [
        "token-secret-artifact",
        "account-secret-artifact@example.invalid",
        r"C:\Users\Alice\private\artifact.png",
        "127.0.0.1:16384",
        &format!("sha256:{}", "d".repeat(64)),
    ] {
        let issued = artifact(secret.as_bytes());
        let json = serde_json::to_string(issued.reference()).expect("artifact JSON");
        let debug = format!("{:?}", issued.reference());
        assert!(!json.contains(secret), "artifact metadata leaked {secret}");
        assert!(!debug.contains(secret), "artifact debug leaked {secret}");
    }
}

#[test]
fn artifact_transport_rejects_every_mutated_field_and_false_store_state() {
    let issued = artifact(b"trusted stored bytes");
    let original = serde_json::to_value(issued.reference()).expect("artifact value");
    let canonical = "11".repeat(16);
    let cases = [
        (
            "artifact_id",
            serde_json::json!(format!("artifact_{canonical}")),
        ),
        ("kind", serde_json::json!("token-secret-kind")),
        ("object_key", serde_json::json!("account-secret/object.png")),
        ("media_type", serde_json::json!("application/token-secret")),
        (
            "sha256",
            serde_json::json!(format!("sha256:{}", "d".repeat(64))),
        ),
        ("producer", serde_json::json!("token-secret-producer")),
        ("retention_class", serde_json::json!("debug_full")),
        ("redaction_state", serde_json::json!("applied")),
    ];

    for (field, replacement) in cases {
        let mut mutated = original.clone();
        mutated[field] = replacement;
        let rendered = mutated.to_string();
        let error = serde_json::from_value::<ArtifactReference>(mutated)
            .expect_err("mutated artifact must fail");
        assert!(!error.to_string().contains("account-secret"));
        assert!(!error.to_string().contains("token-secret"));
        assert!(!format!("{error:?}").contains(&rendered));
    }

    let mut unknown = original;
    unknown["smuggled"] = serde_json::json!("token-secret-unknown");
    assert!(serde_json::from_value::<ArtifactReference>(unknown).is_err());

    let mut undocumented = serde_json::to_value(issued.reference()).expect("artifact value");
    undocumented["store_authorization"] = serde_json::json!(format!("sha256:{}", "e".repeat(64)));
    assert!(serde_json::from_value::<ArtifactReference>(undocumented).is_err());
}

#[test]
fn artifact_wire_shape_has_no_store_authorization() {
    let issued = artifact(b"capture bytes");
    let value = serde_json::to_value(issued.reference()).expect("artifact value");

    assert!(
        value.get("store_authorization").is_none(),
        "artifact wire shape must not claim provenance with store_authorization"
    );
}

#[test]
fn coherent_public_artifact_metadata_mutation_round_trips_without_provenance_claim() {
    let issued = artifact(b"trusted stored bytes");
    let mut value = serde_json::to_value(issued.reference()).expect("artifact value");
    assert!(
        value.get("store_authorization").is_none(),
        "artifact wire shape must not claim provenance with store_authorization"
    );

    let sha256 = format!("sha256:{}", "c".repeat(64));
    let artifact_id = value["artifact_id"]
        .as_str()
        .expect("artifact id")
        .to_string();
    value["byte_count"] = serde_json::json!(999_u64);
    value["sha256"] = serde_json::json!(sha256.clone());
    value["object_key"] =
        serde_json::json!(format!("artifacts/{}/{}.png", &sha256[7..9], artifact_id));

    let round_trip: ArtifactReference =
        serde_json::from_value(value).expect("coherent public metadata must stay typed");
    assert_eq!(round_trip.byte_count(), 999);
    assert_eq!(round_trip.sha256(), sha256);
    assert!(round_trip.object_key().ends_with(".png"));
}

#[test]
fn tagged_payload_and_projection_layers_reject_unknown_fields() {
    let sanitized = sanitize(
        ClientPayloadDraft::cli_command(EventAction::RuntimeStatus, AuditInput::new()).into(),
        1,
    );
    let payload = serde_json::to_value(sanitized.payload()).expect("payload value");

    let mut event_layer = payload.clone();
    event_layer["smuggled"] = serde_json::json!("token-secret-event");
    let error = serde_json::from_value::<EventPayload>(event_layer)
        .expect_err("unknown event payload field");
    assert!(!error.to_string().contains("token-secret"));

    let mut family_layer = payload.clone();
    family_layer["payload"]["smuggled"] = serde_json::json!("token-secret-family");
    let error = serde_json::from_value::<EventPayload>(family_layer)
        .expect_err("unknown family payload field");
    assert!(!error.to_string().contains("token-secret"));

    let mut detail_layer = payload.clone();
    detail_layer["payload"]["data"]["smuggled"] = serde_json::json!("token-secret-detail");
    let error = serde_json::from_value::<EventPayload>(detail_layer)
        .expect_err("unknown payload detail field");
    assert!(!error.to_string().contains("token-secret"));

    let mut audit_layer = payload.clone();
    audit_layer["payload"]["data"]["audit"]["smuggled"] = serde_json::json!("token-secret-audit");
    let error =
        serde_json::from_value::<EventPayload>(audit_layer).expect_err("unknown audit field");
    assert!(!error.to_string().contains("token-secret"));

    let mut projection_layer = serde_json::json!({
        "detail": "full",
        "payload": payload,
    });
    projection_layer["smuggled"] = serde_json::json!("token-secret-projection");
    let error = serde_json::from_value::<ProjectionPayload>(projection_layer)
        .expect_err("unknown projection payload field");
    assert!(!error.to_string().contains("token-secret"));

    let public_projection = ProjectionPayload::Public(sanitized.payload().public_projection());
    let mut public_family_layer =
        serde_json::to_value(&public_projection).expect("public projection value");
    public_family_layer["payload"]["smuggled"] = serde_json::json!("token-secret-public-family");
    let error = serde_json::from_value::<ProjectionPayload>(public_family_layer)
        .expect_err("unknown public family field");
    assert!(!error.to_string().contains("token-secret"));

    let mut public_detail_layer =
        serde_json::to_value(&public_projection).expect("public projection value");
    public_detail_layer["payload"]["payload"]["smuggled"] =
        serde_json::json!("token-secret-public-detail");
    let error = serde_json::from_value::<ProjectionPayload>(public_detail_layer)
        .expect_err("unknown public payload detail field");
    assert!(!error.to_string().contains("token-secret"));
}

#[test]
fn event_v2_round_trips_every_c1_payload_variant() {
    let payloads = all_payload_drafts(AuditInput::new);
    assert_eq!(payloads.len(), 31);

    for (index, payload) in payloads.into_iter().enumerate() {
        let sanitized = sanitize(payload, index as u64 + 1);
        assert_eq!(sanitized.schema_version(), GLOBAL_EVENT_SCHEMA_VERSION);
        assert_eq!(sanitized.event_type(), sanitized.payload().event_type());

        let json = serde_json::to_string(sanitized.payload()).expect("serialize typed payload");
        let round_trip: EventPayload =
            serde_json::from_str(&json).expect("deserialize typed payload");
        assert_eq!(round_trip, *sanitized.payload());
    }
}

#[test]
fn artifact_sha256_matches_known_vector() {
    let issued = artifact(b"abc");
    assert_eq!(
        issued.reference().sha256(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
