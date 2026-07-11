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
        CapturePayloadDraft::requested(EventAction::CaptureObserve, input()).into(),
        CapturePayloadDraft::completed(EventAction::CaptureObserve, effect, 1280, 720, input())
            .into(),
        CapturePayloadDraft::failed(
            EventAction::CaptureObserve,
            DiagnosticCode::CaptureFailed,
            effect,
            input(),
        )
        .into(),
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier2Flush,
            1_000,
            750,
            input(),
        )
        .into(),
        CapturePayloadDraft::dedup_window(3, 900, input()).into(),
        CapturePayloadDraft::policy_changed(
            300,
            RetentionClass::DebugFull,
            CapturePolicyReason::Default,
            input(),
        )
        .into(),
        RecognitionPayloadDraft::requested(EventAction::RecognitionObserve, input()).into(),
        RecognitionPayloadDraft::completed(
            EventAction::RecognitionObserve,
            effect,
            1280,
            720,
            RecognitionVerdict::FrameDecoded,
            input(),
        )
        .into(),
        RecognitionPayloadDraft::failed(
            EventAction::RecognitionObserve,
            DiagnosticCode::RecognitionFailed,
            effect,
            input(),
        )
        .into(),
        ArtifactPayloadDraft::created(input()).into(),
        ArtifactPayloadDraft::verified(input()).into(),
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Success,
            EvidenceCompleteness::Complete,
            2,
            input(),
        )
        .into(),
        ArtifactPayloadDraft::export_failed(
            DiagnosticCode::ArtifactExportFailed,
            TaskOutcome::Failure,
            EvidenceCompleteness::Failed,
            1,
            input(),
        )
        .into(),
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
    type AuditBuilder = fn(&str) -> AuditInput;
    let cases: [(&str, AuditBuilder); 4] = [
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
fn artifact_wire_shape_has_no_store_authorization() {
    let issued = artifact(b"capture bytes");
    let value = serde_json::to_value(issued.reference()).expect("artifact value");
    let mut keys = value
        .as_object()
        .expect("artifact object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();

    assert!(
        value.get("store_authorization").is_none(),
        "artifact wire shape must not claim provenance with store_authorization"
    );
    assert_eq!(
        keys,
        [
            "artifact_id",
            "byte_count",
            "correlation_id",
            "created_at_unix_ms",
            "frame_id",
            "kind",
            "media_type",
            "object_key",
            "producer",
            "redaction_state",
            "retention_class",
            "run_id",
            "sha256",
        ]
    );
}

#[test]
fn artifact_projection_controls_object_key_and_rejects_unknown_fields() {
    let issued = artifact(b"trusted stored bytes");
    let hidden = issued.reference().project(false);
    let visible = issued.reference().project(true);

    assert_eq!(hidden.object_key, None);
    assert_eq!(
        visible.object_key.as_deref(),
        Some(issued.reference().object_key())
    );

    let mut value = serde_json::to_value(visible).expect("artifact projection value");
    value["smuggled"] = serde_json::json!("token-secret-projection-artifact");
    let error = serde_json::from_value::<ProjectedArtifactReference>(value)
        .expect_err("unknown artifact projection field");
    assert!(!error.to_string().contains("token-secret"));
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
    assert_eq!(payloads.len(), 44);

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
fn capture_and_recognition_public_projection_preserve_typed_observation() {
    let capture = sanitize(
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let recognition = sanitize(
        RecognitionPayloadDraft::completed(
            EventAction::RecognitionObserve,
            EffectDisposition::Performed,
            1280,
            720,
            RecognitionVerdict::FrameDecoded,
            AuditInput::new(),
        )
        .into(),
        2,
    );

    for event in [&capture, &recognition] {
        let projection = event.payload().public_projection();
        let json = serde_json::to_value(projection).expect("public projection");
        assert_eq!(json["payload"]["frame_width"], 1280);
        assert_eq!(json["payload"]["frame_height"], 720);
    }
    let projection = recognition.payload().public_projection();
    let json = serde_json::to_value(projection).expect("recognition projection");
    assert_eq!(json["payload"]["recognition_verdict"], "frame_decoded");
}

#[test]
fn artifact_sha256_matches_known_vector() {
    let issued = artifact(b"abc");
    assert_eq!(
        issued.reference().sha256(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn c2_artifact_issuer_preserves_store_owned_metadata() {
    let identifiers = identifier_issuer();
    let issuer = ArtifactStoreIssuer::new().expect("artifact issuer");
    let issued = issuer
        .issue(
            ArtifactKind::EvidenceArchive,
            artifact_links(&identifiers),
            b"evidence archive bytes",
            1_752_147_200_001,
            ArtifactIssuePolicy::new(
                ArtifactProducer::EvidenceExporter,
                RetentionClass::DebugFull,
                ArtifactRedactionState::Applied,
            ),
        )
        .expect("issued artifact");

    let reference = issued.reference();
    assert_eq!(reference.kind(), ArtifactKind::EvidenceArchive);
    assert_eq!(reference.media_type(), ArtifactMediaType::ApplicationZip);
    assert_eq!(reference.producer(), ArtifactProducer::EvidenceExporter);
    assert_eq!(reference.retention_class(), RetentionClass::DebugFull);
    assert_eq!(reference.redaction_state(), ArtifactRedactionState::Applied);
    assert!(reference.object_key().ends_with(".zip"));
    reference.validate().expect("valid reference");
}

#[test]
fn c2_capture_pipeline_and_export_projection_preserve_typed_facts() {
    let pressure = sanitize(
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier3Paused,
            10_000,
            9_500,
            AuditInput::new(),
        )
        .into(),
        1,
    );
    let dedup = sanitize(
        CapturePayloadDraft::dedup_window(4, 1_200, AuditInput::new()).into(),
        2,
    );
    let policy = sanitize(
        CapturePayloadDraft::policy_changed(
            300,
            RetentionClass::DebugFull,
            CapturePolicyReason::RequestOverride,
            AuditInput::new(),
        )
        .into(),
        3,
    );
    let export = sanitize(
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Cancelled,
            EvidenceCompleteness::Partial,
            5,
            AuditInput::new(),
        )
        .into(),
        4,
    );

    let pressure =
        serde_json::to_value(pressure.payload().public_projection()).expect("pressure projection");
    assert_eq!(
        pressure["payload"]["capture_pressure_state"],
        "tier3_paused"
    );
    assert_eq!(pressure["payload"]["memory_budget_bytes"], 10_000);
    assert_eq!(pressure["payload"]["resident_bytes"], 9_500);

    let dedup =
        serde_json::to_value(dedup.payload().public_projection()).expect("dedup projection");
    assert_eq!(dedup["payload"]["duplicate_count"], 4);
    assert_eq!(dedup["payload"]["duration_ms"], 1_200);

    let policy =
        serde_json::to_value(policy.payload().public_projection()).expect("policy projection");
    assert_eq!(policy["payload"]["cadence_ms"], 300);
    assert_eq!(policy["payload"]["retention_class"], "debug_full");
    assert_eq!(
        policy["payload"]["capture_policy_reason"],
        "request_override"
    );

    let export =
        serde_json::to_value(export.payload().public_projection()).expect("export projection");
    assert_eq!(export["family"], "artifact");
    assert_eq!(export["payload"]["task_outcome"], "cancelled");
    assert_eq!(export["payload"]["evidence_completeness"], "partial");
    assert_eq!(export["payload"]["artifact_count"], 5);
}

#[test]
fn c2_payloads_reject_invalid_numeric_and_unknown_closed_values() {
    for payload in [
        CapturePayloadDraft::pressure_changed(
            CapturePressureState::Tier1Dedup,
            0,
            0,
            AuditInput::new(),
        )
        .into(),
        CapturePayloadDraft::dedup_window(0, 300, AuditInput::new()).into(),
        CapturePayloadDraft::policy_changed(
            0,
            RetentionClass::Adaptive,
            CapturePolicyReason::Default,
            AuditInput::new(),
        )
        .into(),
        ArtifactPayloadDraft::export_completed(
            TaskOutcome::Success,
            EvidenceCompleteness::Complete,
            0,
            AuditInput::new(),
        )
        .into(),
    ] {
        let issuer = identifier_issuer();
        let error = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            links(&issuer),
            payload,
        )
        .sanitize(&SpyFingerprinter::new())
        .expect_err("invalid C2 payload");
        assert!(error.code().starts_with("invalid_"));
    }

    let value = serde_json::json!({
        "family": "capture",
        "payload": {
            "kind": "pressure_changed",
            "data": {
                "action": "capture.pressure",
                "state": "tier4_impossible",
                "memory_budget_bytes": 1000,
                "resident_bytes": 900,
                "audit": {
                    "account_fingerprint": null,
                    "authentication_fingerprint": null,
                    "machine_path_present": false,
                    "device_endpoint_present": false
                }
            }
        }
    });
    let error =
        serde_json::from_value::<EventPayload>(value).expect_err("unknown pressure state rejected");
    assert!(!error.to_string().contains("tier4_impossible"));
}
