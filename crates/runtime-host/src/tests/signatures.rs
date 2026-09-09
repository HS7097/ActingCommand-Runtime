// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

// Workflow #257 SIGNATURE-REPLAY-v1: explicit Runtime writer specification.
#[test]
fn signatures_are_lab_owned_explicit_idempotent_operations_without_device_effects() {
    use actingcommand_contract::{
        AuditInput, CommandPayloadDraft, DiagnosticCode, DiagnosticSignatureDefinition, EventDraft,
        EventLinksDraft, EventOrigin, RuntimeSignatureMatchRequest, SignaturePageRequest,
        SignatureReplayGap,
    };
    use actingcommand_ledger::Sha256SecretFingerprinter;
    let input_root = TempDir::new().unwrap();
    let input = GlobalLedger::open(GlobalLedgerConfig::new(
        input_root.path().join("ledger"),
        "signature-source",
    ))
    .unwrap();
    let ids = IdentifierIssuer::new().unwrap();
    input
        .append(
            EventDraft::new(
                ids.mint_event_id().unwrap(),
                1,
                EventSeverity::Error,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                CommandPayloadDraft::rejected(
                    actingcommand_contract::EventAction::RuntimeStart,
                    DiagnosticCode::CommandRejected,
                    EffectDisposition::NotPerformed,
                    AuditInput::new(),
                )
                .into(),
            )
            .sanitize(&Sha256SecretFingerprinter::new(b"host-signature-spec").unwrap())
            .unwrap(),
        )
        .unwrap();
    input.close().unwrap();
    let root = TempDir::new().unwrap();
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let connection = ConnectionId::new(123).unwrap();
    let mut client = TestClient::connect(&host);
    let request = |operation| {
        RuntimeRequest::new(
            ids.mint_request_id().unwrap(),
            ids.mint_correlation_id().unwrap(),
            None,
            EventActor::Lab,
            EventSource::Lab,
            unix_ms_now().unwrap(),
            operation,
        )
        .unwrap()
    };
    let register = request(RuntimeOperation::RegisterDiagnosticSignature {
        definition: Box::new(DiagnosticSignatureDefinition {
            signature_id: "command_rejection".into(),
            version: 1,
            origin_module: OriginModule::Runtime,
            diagnostic_code: DiagnosticCode::CommandRejected,
            event_type: EventType::CommandRejected,
            minimum_severity: EventSeverity::Error,
            lifecycle: None,
        }),
    });
    let first = client.send(&register);
    assert_eq!(first.state(), RuntimeReceiptState::Completed);
    assert_eq!(first, client.send(&register));
    let Some(RuntimeResult::SignatureRegistered { registration }) = first.result() else {
        panic!("registration");
    };
    assert_eq!(first.terminal().unwrap().event_id, registration.event_id);
    assert_eq!(first.terminal().unwrap().sequence, registration.sequence);
    let registration = registration.clone();
    let mut forged = serde_json::to_value(&register).unwrap();
    forged["actor"] = serde_json::json!("cli");
    forged["source"] = serde_json::json!("cli");
    let forged: RuntimeRequest = serde_json::from_value(forged).unwrap();
    assert_eq!(
        host.process_request_for_test(&forged, connection)
            .unwrap()
            .state(),
        RuntimeReceiptState::Denied
    );
    let match_request = |through| {
        request(RuntimeOperation::MatchDiagnosticSignatures {
            request: Box::new(RuntimeSignatureMatchRequest {
                input_state_root: input_root.path().to_str().unwrap().into(),
                input_through: 1,
                catalog_through: through,
                page: SignaturePageRequest::default(),
            }),
        })
    };
    let matched_request = match_request(registration.sequence);
    let matched = client.send(&matched_request);
    assert_eq!(matched.state(), RuntimeReceiptState::Completed);
    assert!(matched.terminal().is_some());
    let Some(RuntimeResult::SignaturesMatched { page }) = matched.result() else {
        panic!("matched page");
    };
    assert_eq!(page.matched_count, 1);
    assert!(page.evidence_complete());
    assert_eq!(matched, client.send(&matched_request));
    for _ in 0..2 {
        let query = runtime_request(
            &ids,
            RuntimeOperation::QueryEvents {
                query: EventQuery {
                    event_type: Some(EventType::SignatureMatched),
                    ..Default::default()
                },
                profile: ProjectionProfile::Forensic,
                page: RuntimeEventQueryPageRequest::default(),
            },
        );
        let receipt = host.process_request_for_test(&query, connection).unwrap();
        let Some(RuntimeResult::EventPage { page }) = receipt.result() else {
            panic!("event page");
        };
        assert_eq!(page.events().len(), 1);
    }
    let retired = host
        .process_request_for_test(
            &request(RuntimeOperation::RetireDiagnosticSignature {
                registration: registration.clone(),
            }),
            connection,
        )
        .unwrap();
    assert_eq!(retired.state(), RuntimeReceiptState::Completed);
    let after = host
        .process_request_for_test(
            &match_request(retired.terminal().unwrap().sequence),
            connection,
        )
        .unwrap();
    let Some(RuntimeResult::SignaturesMatched { page }) = after.result() else {
        panic!("retired page");
    };
    assert_eq!(page.matched_count, 0);
    assert!(!page.evidence_complete());
    assert!(page.gaps.contains(&SignatureReplayGap::CatalogEmpty));
    let frozen = host
        .process_request_for_test(&match_request(registration.sequence), connection)
        .unwrap();
    let Some(RuntimeResult::SignaturesMatched { page }) = frozen.result() else {
        panic!("frozen page");
    };
    assert_eq!(page.matched_count, 1);
    assert!(page.evidence_complete());
    for counter in [
        &state.open_count,
        &state.input_count,
        &state.capture_open_count,
        &state.capture_count,
    ] {
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
    drop(client);
    host.close().unwrap();
}
