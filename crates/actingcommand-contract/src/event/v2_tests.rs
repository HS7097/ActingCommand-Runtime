use super::*;

struct TestFingerprinter;

impl SecretFingerprinter for TestFingerprinter {
    fn fingerprint(
        &self,
        field: SecretField,
        original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        let digit = match field {
            SecretField::AccountIdentity => 'a',
            SecretField::AuthenticationMaterial => 'b',
        };
        Sha256Fingerprint::new(format!("sha256:{}", digit.to_string().repeat(64)), original)
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

fn code(value: &'static str) -> StaticCode {
    StaticCode::new(value).expect("static code")
}

fn audit() -> AuditInput {
    AuditInput::new()
        .with_account("account-secret-c1@example.invalid")
        .with_authentication("authentication-secret-c1")
        .with_machine_path(r"C:\Users\Alice\private\runtime.json")
        .with_device_endpoint("127.0.0.1:16384")
}

fn origin() -> EventOrigin {
    EventOrigin::new(EventSource::Cli, code("actingctl"), EventActor::User)
}

fn links() -> EventLinks {
    EventLinks::default()
        .with_instance_id(InstanceId::new([1; 16]))
        .with_request_id(RequestId::new([2; 16]))
        .with_correlation_id(CorrelationId::new([3; 16]))
        .with_causation_id(CausationId::new([4; 16]))
        .with_task_id(TaskId::new([5; 16]))
        .with_run_id(RunId::new([6; 16]))
        .with_lease_id(LeaseId::new([7; 16]))
        .with_frame_id(FrameId::new([8; 16]))
        .with_action_id(ActionId::new([9; 16]))
        .with_recognition_id(RecognitionId::new([10; 16]))
}

fn artifact(object_key: &str) -> Result<ArtifactReference, SanitizationError> {
    ArtifactReference::new(
        ArtifactId::new([11; 16]),
        code("capture.frame"),
        Some(RunId::new([6; 16])),
        Some(FrameId::new([8; 16])),
        Some(CorrelationId::new([3; 16])),
        object_key,
        "image/png",
        4096,
        format!("sha256:{}", "c".repeat(64)),
        1_752_147_200_000,
        code("capture-store"),
        RetentionClass::Adaptive,
        ArtifactRedactionState::Applied,
    )
}

fn all_payload_drafts(with_audit: bool) -> Vec<EventPayloadDraft> {
    let input = || {
        if with_audit {
            audit()
        } else {
            AuditInput::new()
        }
    };
    let action = || code("runtime.action");
    let diagnostic = || code("runtime.diagnostic");
    let effect = EffectDisposition::Performed;

    vec![
        CommandPayloadDraft::received(action(), input()).into(),
        CommandPayloadDraft::validated(action(), effect, input()).into(),
        CommandPayloadDraft::rejected(action(), diagnostic(), effect, input()).into(),
        SchedulerPayloadDraft::admitted(action(), input()).into(),
        SchedulerPayloadDraft::queued(action(), input()).into(),
        SchedulerPayloadDraft::denied(action(), diagnostic(), input()).into(),
        SchedulerPayloadDraft::preempted(action(), diagnostic(), input()).into(),
        LeasePayloadDraft::requested(action(), input()).into(),
        LeasePayloadDraft::granted(action(), effect, input()).into(),
        LeasePayloadDraft::transferred(action(), effect, input()).into(),
        LeasePayloadDraft::released(action(), effect, input()).into(),
        LeasePayloadDraft::expired(action(), effect, input()).into(),
        LeasePayloadDraft::transition_intent(action(), input()).into(),
        LeasePayloadDraft::transition_failed(action(), diagnostic(), effect, input()).into(),
        TaskPayloadDraft::requested(action(), input()).into(),
        TaskPayloadDraft::started(action(), input()).into(),
        TaskPayloadDraft::step_started(action(), input()).into(),
        TaskPayloadDraft::step_finished(action(), input()).into(),
        TaskPayloadDraft::completed(action(), effect, input()).into(),
        TaskPayloadDraft::failed(action(), diagnostic(), effect, input()).into(),
        TaskPayloadDraft::cancelled(action(), effect, input()).into(),
        TaskPayloadDraft::terminal_intent(action(), input()).into(),
        TaskPayloadDraft::terminal_commit_failed(action(), diagnostic(), effect, input()).into(),
        InputPayloadDraft::intent(action(), input()).into(),
        InputPayloadDraft::committed(action(), effect, input()).into(),
        InputPayloadDraft::completed(action(), input()).into(),
        InputPayloadDraft::failed(action(), diagnostic(), effect, input()).into(),
        ClientPayloadDraft::ui_action(action(), input()).into(),
        ClientPayloadDraft::cli_command(action(), input()).into(),
        ClientPayloadDraft::lab_request(action(), input()).into(),
        LedgerPayloadDraft::recovered(code("stale_owner"), Some(1), 64, input()).into(),
    ]
}

fn sanitize(payload: EventPayloadDraft, index: u8) -> SanitizedEventDraft {
    EventDraft::new(
        EventId::new([index; 16]),
        1_752_147_200_000 + u64::from(index),
        EventSeverity::Info,
        origin(),
        links(),
        payload,
    )
    .sanitize(&TestFingerprinter)
    .expect("sanitize event")
}

#[test]
fn producer_cannot_select_redaction_policy() {
    let sanitized = sanitize(
        CommandPayloadDraft::received(code("runtime.start"), audit()).into(),
        1,
    );
    let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

    assert!(!json.contains("redaction_policy"));
    assert!(!json.contains("\"policy\""));
}

#[test]
fn all_runtime_secret_classes_follow_schema_owned_policy() {
    for (index, payload) in all_payload_drafts(true).into_iter().enumerate() {
        let sanitized = sanitize(payload, u8::try_from(index + 1).expect("test index"));
        let json = serde_json::to_string(&sanitized).expect("serialize sanitized event");

        for original in [
            "account-secret-c1@example.invalid",
            "authentication-secret-c1",
            r"C:\Users\Alice\private\runtime.json",
            "127.0.0.1:16384",
        ] {
            assert!(!json.contains(original), "secret leaked: {original}");
        }
        assert!(json.contains(&format!("sha256:{}", "a".repeat(64))));
        assert!(json.contains("[redacted]"));
        assert!(json.contains("authentication_redacted"));
    }
}

#[test]
fn hash_shaped_original_cannot_survive_as_fingerprint() {
    for original in ["a".repeat(64), format!("sha256:{}", "a".repeat(64))] {
        let draft = EventDraft::new(
            EventId::new([1; 16]),
            1_752_147_200_000,
            EventSeverity::Info,
            origin(),
            EventLinks::default(),
            CommandPayloadDraft::received(
                code("runtime.start"),
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
fn runtime_values_cannot_enter_static_code_or_typed_ids() {
    assert!(StaticCode::new("runtime.valid").is_ok());
    assert!(StaticCode::new(".").is_err());
    assert!(StaticCode::new("-runtime").is_err());
    assert!(serde_json::from_str::<StaticCode>(r#""C:\\private\\module""#).is_err());
    assert!(serde_json::from_str::<StaticCode>(r#""127.0.0.1:16384""#).is_err());
    assert!(serde_json::from_str::<RequestId>(r#""request_runtime-value""#).is_err());

    let encoded = serde_json::to_string(&EventId::new([0xab; 16])).expect("serialize event id");
    assert_eq!(encoded, format!("\"evt_{}\"", "ab".repeat(16)));
}

#[test]
fn sanitized_event_is_typed_and_not_deserializable() {
    let sanitized = sanitize(
        CommandPayloadDraft::received(code("runtime.start"), AuditInput::new()).into(),
        1,
    );

    assert!(matches!(
        sanitized.payload(),
        EventPayload::Command(CommandPayload::Received(_))
    ));
    assert_eq!(sanitized.event_type(), EventType::CommandReceived);
}

#[test]
fn artifact_reference_requires_complete_v3_metadata() {
    let artifact = artifact("runs/run-1/frame-1.png").expect("complete artifact");

    assert_eq!(artifact.artifact_id(), &ArtifactId::new([11; 16]));
    assert_eq!(artifact.kind(), &code("capture.frame"));
    assert_eq!(artifact.run_id(), Some(&RunId::new([6; 16])));
    assert_eq!(artifact.frame_id(), Some(&FrameId::new([8; 16])));
    assert_eq!(
        artifact.correlation_id(),
        Some(&CorrelationId::new([3; 16]))
    );
    assert_eq!(artifact.object_key(), "runs/run-1/frame-1.png");
    assert_eq!(artifact.media_type(), "image/png");
    assert_eq!(artifact.byte_count(), 4096);
    assert_eq!(artifact.created_at_unix_ms(), 1_752_147_200_000);
    assert_eq!(artifact.producer(), &code("capture-store"));
    assert_eq!(artifact.retention_class(), RetentionClass::Adaptive);
    assert_eq!(artifact.redaction_state(), ArtifactRedactionState::Applied);

    let mut value = serde_json::to_value(&artifact).expect("artifact value");
    value
        .as_object_mut()
        .expect("artifact object")
        .remove("media_type");
    assert!(serde_json::from_value::<ArtifactReference>(value).is_err());

    let mut value = serde_json::to_value(&artifact).expect("artifact value");
    value["media_type"] = serde_json::json!("image/png/extra");
    assert!(serde_json::from_value::<ArtifactReference>(value).is_err());
}

#[test]
fn artifact_object_key_rejects_absolute_or_parent_paths() {
    for invalid in [
        "/absolute/frame.png",
        r"C:\absolute\frame.png",
        "../frame.png",
        "runs/../frame.png",
        r"runs\frame.png",
        "runs/private frame.png",
        "runs/private\0frame.png",
        "runs/私密/frame.png",
    ] {
        let error = artifact(invalid).expect_err("unsafe object key must fail");
        assert_eq!(error.code(), "invalid_artifact_object_key");
        assert!(!error.to_string().contains(invalid));
    }
}

#[test]
fn event_v2_round_trips_every_c1_payload_variant() {
    let payloads = all_payload_drafts(false);
    assert_eq!(payloads.len(), 31);

    for (index, payload) in payloads.into_iter().enumerate() {
        let sanitized = sanitize(payload, u8::try_from(index + 1).expect("test index"));
        assert_eq!(sanitized.schema_version(), GLOBAL_EVENT_SCHEMA_VERSION);
        assert_eq!(sanitized.event_type(), sanitized.payload().event_type());

        let json = serde_json::to_string(sanitized.payload()).expect("serialize typed payload");
        let round_trip: EventPayload =
            serde_json::from_str(&json).expect("deserialize typed payload");
        assert_eq!(round_trip, *sanitized.payload());
    }
}
