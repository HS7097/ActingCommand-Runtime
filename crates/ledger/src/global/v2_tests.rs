use super::*;
use actingcommand_contract::{
    ArtifactId, ArtifactRedactionState, ArtifactReference, AuditInput, ClientPayloadDraft,
    CorrelationId, EventActor, EventDraft, EventId, EventLinks, EventOrigin, EventQuery,
    EventSeverity, EventSource, ProjectionPayload, RetentionClass, RunId, StaticCode,
};
use std::fs;
use tempfile::TempDir;

fn code(value: &'static str) -> StaticCode {
    StaticCode::new(value).expect("static code")
}

fn config(temp: &TempDir, owner: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(temp.path(), owner)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn artifact() -> ArtifactReference {
    ArtifactReference::new(
        ArtifactId::new([7; 16]),
        code("capture.frame"),
        Some(RunId::new([8; 16])),
        None,
        Some(CorrelationId::new([9; 16])),
        "runs/run-8/frame.png",
        "image/png",
        32,
        format!("sha256:{}", "d".repeat(64)),
        1_752_147_200_000,
        code("capture-store"),
        RetentionClass::Adaptive,
        ArtifactRedactionState::Applied,
    )
    .expect("artifact")
}

fn event(index: u8, with_artifact: bool) -> actingcommand_contract::SanitizedEventDraft {
    let payload = ClientPayloadDraft::cli_command(
        code("runtime.status"),
        AuditInput::new()
            .with_account("account-secret-v2")
            .with_authentication("token-secret-v2")
            .with_machine_path(r"C:\private\runtime.json")
            .with_device_endpoint("127.0.0.1:16384"),
    );
    let draft = EventDraft::new(
        EventId::new([index; 16]),
        1_752_147_200_000 + u64::from(index),
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, code("actingctl"), EventActor::User),
        EventLinks::default().with_correlation_id(CorrelationId::new([9; 16])),
        payload.into(),
    );
    let draft = if with_artifact {
        draft.with_artifacts(vec![artifact()])
    } else {
        draft
    };
    draft
        .sanitize(&Sha256SecretFingerprinter::new(b"ledger-v2-test-salt").expect("fingerprinter"))
        .expect("sanitize")
}

#[test]
fn persisted_event_cannot_be_constructed_or_deserialized_by_consumers() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-fact")).expect("ledger");
    let persisted = ledger.append(event(1, false)).expect("append");
    let serialized = serde_json::to_value(&persisted).expect("serialize persisted fact");

    assert_eq!(persisted.sequence(), 1);
    assert_eq!(serialized["sequence"], 1);
    assert_eq!(persisted.event_id(), &EventId::new([1; 16]));
}

#[test]
fn storage_assigns_the_only_sequence() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-sequence")).expect("ledger");

    let first = ledger.append(event(1, false)).expect("first append");
    let second = ledger.append(event(2, false)).expect("second append");

    assert_eq!(first.sequence(), 1);
    assert_eq!(second.sequence(), 2);
}

#[test]
fn v1_generic_segment_fails_loudly() {
    let temp = TempDir::new().expect("temp");
    let segments = temp.path().join("segments");
    fs::create_dir_all(&segments).expect("segments");
    fs::write(
        segments.join("segment-000001.jsonl"),
        b"{\"line_type\":\"event\",\"event\":{\"schema_version\":\"actingcommand.event.v1\",\"payload\":{\"fields\":[]}}}\n",
    )
    .expect("legacy segment");

    let error = GlobalLedger::open(config(&temp, "writer-v1")).expect_err("v1 segment must fail");

    assert_eq!(error.code(), "unsupported_event_schema");
    assert!(error.is_fatal());
}

#[test]
fn typed_record_recovery_rebuilds_same_fact() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-first")).expect("ledger");
    let expected = ledger.append(event(1, true)).expect("append");
    ledger.close().expect("close");

    let reopened = GlobalLedger::open(config(&temp, "writer-second")).expect("reopen");
    let recovered = reopened.query(EventQuery::default()).expect("query");

    assert_eq!(recovered, vec![expected]);
}

#[test]
fn concise_projection_omits_payload() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-concise")).expect("ledger");
    ledger.append(event(1, false)).expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Concise)
        .expect("project");

    assert!(matches!(projected[0].payload, ProjectionPayload::Omitted));
}

#[test]
fn ui_projection_contains_only_public_typed_payload() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-ui")).expect("ledger");
    ledger.append(event(1, false)).expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("project");
    let json = serde_json::to_string(&projected[0]).expect("serialize projection");

    assert!(matches!(projected[0].payload, ProjectionPayload::Public(_)));
    for forbidden in [
        "audit",
        "account-secret-v2",
        "token-secret-v2",
        r"C:\private\runtime.json",
        "127.0.0.1:16384",
        "account_fingerprint",
        "authentication_redacted",
    ] {
        assert!(
            !json.contains(forbidden),
            "UI projection leaked {forbidden}"
        );
    }
}

#[test]
fn lab_projection_contains_full_sanitized_typed_payload() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-lab")).expect("ledger");
    let persisted = ledger.append(event(1, false)).expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("project");

    assert_eq!(
        projected[0].payload,
        ProjectionPayload::Full(persisted.payload().clone())
    );
}

#[test]
fn ui_projection_omits_artifact_object_key() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-artifact-ui")).expect("ledger");
    ledger.append(event(1, true)).expect("append");

    let ui = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("UI projection");
    let lab = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("Lab projection");

    assert_eq!(ui[0].artifacts[0].object_key, None);
    assert_eq!(
        lab[0].artifacts[0].object_key.as_deref(),
        Some("runs/run-8/frame.png")
    );
}
