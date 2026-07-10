use super::*;
use actingcommand_contract::{
    ArtifactRedactionState, AuditInput, ClientPayloadDraft, CorrelationId, EventAction, EventActor,
    EventDraft, EventId, EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource,
    IdentifierIssuer, OriginModule, ProjectedEvent, ProjectionPayload, Sensitivity,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use tempfile::TempDir;

const ARTIFACT_SECRETS: [&str; 5] = [
    "token-secret-artifact-ledger",
    "account-secret-artifact-ledger@example.invalid",
    r"C:\Users\Alice\private\artifact.png",
    "127.0.0.1:16384",
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
];

struct EventFixture {
    draft: actingcommand_contract::SanitizedEventDraft,
    event_id: EventId,
    correlation_id: CorrelationId,
}

fn identifier_issuer() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn config(temp: &TempDir, owner: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(temp.path(), owner)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn event(index: u64) -> EventFixture {
    let identifiers = identifier_issuer();
    let event_id = identifiers.mint_event_id().expect("event id");
    let expected_event_id = *event_id.transport();
    let correlation_id = identifiers.mint_correlation_id().expect("correlation id");
    let expected_correlation_id = *correlation_id.transport();
    let payload = ClientPayloadDraft::cli_command(
        EventAction::RuntimeStatus,
        AuditInput::new()
            .with_account("account-secret-v2")
            .with_authentication("token-secret-v2")
            .with_machine_path(r"C:\private\runtime.json")
            .with_device_endpoint("127.0.0.1:16384"),
    );
    let draft = EventDraft::new(
        event_id,
        1_752_147_200_000 + index,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default().with_correlation_id(correlation_id),
        payload.into(),
    );
    EventFixture {
        draft: draft
            .sanitize(
                &Sha256SecretFingerprinter::new(b"ledger-v2-test-salt").expect("fingerprinter"),
            )
            .expect("sanitize"),
        event_id: expected_event_id,
        correlation_id: expected_correlation_id,
    }
}

fn canonical_id<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("serialize id")
        .as_str()
        .expect("canonical id")
        .to_string()
}

fn canonical_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn inject_artifact_record(temp: &TempDir, correlation_id: CorrelationId, bytes: &[u8]) {
    let identifiers = identifier_issuer();
    let artifact_id = canonical_id(
        *identifiers
            .mint_artifact_id()
            .expect("artifact id")
            .transport(),
    );
    let run_id = canonical_id(*identifiers.mint_run_id().expect("run id").transport());
    let frame_id = canonical_id(*identifiers.mint_frame_id().expect("frame id").transport());
    let sha256 = canonical_sha256(bytes);
    let object_key = format!("artifacts/{}/{}.png", &sha256[7..9], artifact_id);
    let segment_path = temp.path().join("segments/segment-000001.jsonl");
    let source = fs::read_to_string(&segment_path).expect("segment");
    let mut line: Value = serde_json::from_str(source.trim_end()).expect("stored line");
    line["event"]["sensitivity"] = Value::String("secret".to_string());
    line["event"]["artifacts"] = serde_json::json!([{
        "artifact_id": artifact_id,
        "kind": "capture.frame",
        "run_id": run_id,
        "frame_id": frame_id,
        "correlation_id": correlation_id,
        "object_key": object_key,
        "media_type": "image/png",
        "byte_count": u64::try_from(bytes.len()).expect("byte count"),
        "sha256": sha256,
        "created_at_unix_ms": 1_752_147_200_000u64,
        "producer": "capture_store",
        "retention_class": "adaptive",
        "redaction_state": "pending"
    }]);
    fs::write(
        &segment_path,
        format!("{}\n", serde_json::to_string(&line).expect("artifact line")),
    )
    .expect("write artifact line");
}

fn seed_recovered_artifact_event(temp: &TempDir, owner: &str, index: u64) -> EventFixture {
    let fixture = event(index);
    let ledger = GlobalLedger::open(config(temp, owner)).expect("ledger");
    ledger.append(fixture.draft.clone()).expect("append");
    ledger.close().expect("close");
    inject_artifact_record(
        temp,
        fixture.correlation_id,
        ARTIFACT_SECRETS.join("|").as_bytes(),
    );
    fixture
}

#[test]
fn persisted_event_cannot_be_constructed_or_deserialized_by_consumers() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-fact")).expect("ledger");
    let fixture = event(1);
    let expected_id = fixture.event_id;
    let persisted = ledger.append(fixture.draft).expect("append");
    let serialized = serde_json::to_value(&persisted).expect("serialize persisted fact");

    assert_eq!(persisted.sequence(), 1);
    assert_eq!(serialized["sequence"], 1);
    assert_eq!(persisted.event_id(), &expected_id);
}

#[test]
fn storage_assigns_the_only_sequence() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-sequence")).expect("ledger");

    let first = ledger.append(event(1).draft).expect("first append");
    let second = ledger.append(event(2).draft).expect("second append");

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
    let fixture = seed_recovered_artifact_event(&temp, "writer-first", 1);
    let reopened = GlobalLedger::open(config(&temp, "writer-second")).expect("reopen");
    let recovered = reopened.query(EventQuery::default()).expect("query");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].event_id(), &fixture.event_id);
    assert_eq!(
        recovered[0].links().correlation_id(),
        Some(&fixture.correlation_id)
    );
    assert_eq!(recovered[0].artifacts().len(), 1);
}

#[test]
fn concise_projection_omits_payload() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-concise")).expect("ledger");
    ledger.append(event(1).draft).expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Concise)
        .expect("project");

    assert!(matches!(projected[0].payload, ProjectionPayload::Omitted));
}

#[test]
fn ui_projection_contains_only_public_typed_payload() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-ui")).expect("ledger");
    ledger.append(event(1).draft).expect("append");

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
    let persisted = ledger.append(event(1).draft).expect("append");

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
    seed_recovered_artifact_event(&temp, "writer-artifact-ui", 1);
    let ledger = GlobalLedger::open(config(&temp, "writer-artifact-ui-reopen")).expect("ledger");

    let ui = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("UI projection");
    let lab = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("Lab projection");

    assert_eq!(ui[0].artifacts[0].object_key, None);
    assert!(
        lab[0].artifacts[0]
            .object_key
            .as_deref()
            .is_some_and(|key| key.starts_with("artifacts/"))
    );
}

#[test]
fn artifact_bytes_and_metadata_are_safe_across_persistence_query_and_every_projection() {
    let temp = TempDir::new().expect("temp");
    let fixture = seed_recovered_artifact_event(&temp, "writer-artifact", 1);
    let correlation_id = fixture.correlation_id;
    let ledger = GlobalLedger::open(config(&temp, "writer-artifact-reopen")).expect("ledger");
    let persisted = ledger
        .query(EventQuery::default())
        .expect("query")
        .remove(0);

    assert_eq!(persisted.sensitivity(), Sensitivity::Secret);
    assert_eq!(
        persisted.artifacts()[0].redaction_state(),
        ArtifactRedactionState::Pending
    );
    let query = EventQuery {
        correlation_id: Some(correlation_id),
        ..EventQuery::default()
    };
    let queried = ledger.query(query.clone()).expect("query artifact event");
    assert_eq!(queried, vec![persisted.clone()]);

    let segment =
        fs::read_to_string(temp.path().join("segments/segment-000001.jsonl")).expect("segment");
    let debug = format!("{persisted:?}");
    for secret in ARTIFACT_SECRETS {
        assert!(!segment.contains(secret), "segment leaked {secret}");
        assert!(!debug.contains(secret), "debug leaked {secret}");
    }

    for profile in [
        ProjectionProfile::Cli,
        ProjectionProfile::Ui,
        ProjectionProfile::Lab,
        ProjectionProfile::Concise,
        ProjectionProfile::Normal,
        ProjectionProfile::Verbose,
        ProjectionProfile::Forensic,
    ] {
        let projection = ledger.project(query.clone(), profile).expect("projection");
        let json = serde_json::to_string(&projection).expect("projection JSON");
        for secret in ARTIFACT_SECRETS {
            assert!(!json.contains(secret), "{profile:?} leaked {secret}");
        }
    }
}

#[test]
fn recovery_rejects_unknown_and_inconsistent_v2_layers_without_disclosure() {
    let mutations: [(&str, fn(&mut Value)); 9] = [
        ("event", |line| {
            line["event"]["smuggled"] = Value::String("token-secret-event".to_string());
        }),
        ("payload", |line| {
            line["event"]["payload"]["smuggled"] =
                Value::String("token-secret-payload".to_string());
        }),
        ("family", |line| {
            line["event"]["payload"]["payload"]["smuggled"] =
                Value::String("token-secret-family".to_string());
        }),
        ("detail", |line| {
            line["event"]["payload"]["payload"]["data"]["smuggled"] =
                Value::String("token-secret-detail".to_string());
        }),
        ("artifact", |line| {
            line["event"]["artifacts"][0]["smuggled"] =
                Value::String("token-secret-artifact".to_string());
        }),
        ("event_type", |line| {
            line["event"]["event_type"] = Value::String("command.received".to_string());
        }),
        ("payload_schema", |line| {
            line["event"]["payload_schema"] = Value::String("invalid.schema".to_string());
        }),
        ("sensitivity", |line| {
            line["event"]["sensitivity"] = Value::String("public".to_string());
        }),
        ("artifact_hash", |line| {
            line["event"]["artifacts"][0]["sha256"] =
                Value::String(format!("sha256:{}", "f".repeat(64)));
        }),
    ];

    for (label, mutate) in mutations {
        let temp = TempDir::new().expect("temp");
        seed_recovered_artifact_event(&temp, "writer-mutate", 1);
        let segment_path = temp.path().join("segments/segment-000001.jsonl");
        let source = fs::read_to_string(&segment_path).expect("segment");
        let mut line: Value = serde_json::from_str(source.trim_end()).expect("stored line");
        mutate(&mut line);
        fs::write(
            &segment_path,
            format!("{}\n", serde_json::to_string(&line).expect("mutated line")),
        )
        .expect("write mutation");

        let error = GlobalLedger::open(config(&temp, "writer-recover"))
            .expect_err("mutated record must fail");
        assert!(error.is_fatal(), "{label} was not fatal");
        let diagnostic = format!("{error:?} {error}");
        assert!(
            !diagnostic.contains("token-secret"),
            "{label} disclosed value"
        );
    }
}

#[test]
fn coherent_artifact_metadata_mutation_recovers_without_store_authorization_claims() {
    let temp = TempDir::new().expect("temp");
    seed_recovered_artifact_event(&temp, "writer-coherent-artifact", 1);
    let segment_path = temp.path().join("segments/segment-000001.jsonl");
    let source = fs::read_to_string(&segment_path).expect("segment");
    let mut line: Value = serde_json::from_str(source.trim_end()).expect("stored line");
    let artifact = line["event"]["artifacts"][0]
        .as_object_mut()
        .expect("artifact object");
    assert!(
        !artifact.contains_key("store_authorization"),
        "artifact recovery shape must not claim provenance with store_authorization"
    );

    let sha256 = format!("sha256:{}", "c".repeat(64));
    let artifact_id = artifact
        .get("artifact_id")
        .and_then(Value::as_str)
        .expect("artifact id")
        .to_string();
    artifact.insert("byte_count".to_string(), Value::from(999_u64));
    artifact.insert("sha256".to_string(), Value::String(sha256.clone()));
    artifact.insert(
        "object_key".to_string(),
        Value::String(format!("artifacts/{}/{}.png", &sha256[7..9], artifact_id)),
    );
    fs::write(
        &segment_path,
        format!("{}\n", serde_json::to_string(&line).expect("mutated line")),
    )
    .expect("write mutation");

    let reopened = GlobalLedger::open(config(&temp, "writer-coherent-recover"))
        .expect("coherent artifact recovery");
    let recovered = reopened.query(EventQuery::default()).expect("query");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].artifacts()[0].byte_count(), 999);
    assert_eq!(recovered[0].artifacts()[0].sha256(), sha256);
}

#[test]
fn projected_event_rejects_unknown_projection_nesting_without_disclosure() {
    let temp = TempDir::new().expect("temp");
    seed_recovered_artifact_event(&temp, "writer-projection-strict", 1);
    let ledger =
        GlobalLedger::open(config(&temp, "writer-projection-strict-reopen")).expect("ledger");
    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("projection")
        .remove(0);

    let mut event_layer = serde_json::to_value(&projected).expect("projection value");
    event_layer["smuggled"] = Value::String("token-secret-projection-event".to_string());
    let error = serde_json::from_value::<ProjectedEvent>(event_layer)
        .expect_err("unknown projection event field");
    assert!(!error.to_string().contains("token-secret"));

    let mut payload_layer = serde_json::to_value(&projected).expect("projection value");
    payload_layer["payload"]["smuggled"] =
        Value::String("token-secret-projection-payload".to_string());
    let error = serde_json::from_value::<ProjectedEvent>(payload_layer)
        .expect_err("unknown projection payload field");
    assert!(!error.to_string().contains("token-secret"));

    let mut artifact_layer = serde_json::to_value(&projected).expect("projection value");
    artifact_layer["artifacts"][0]["smuggled"] =
        Value::String("token-secret-projection-artifact".to_string());
    let error = serde_json::from_value::<ProjectedEvent>(artifact_layer)
        .expect_err("unknown projection artifact field");
    assert!(!error.to_string().contains("token-secret"));
}
