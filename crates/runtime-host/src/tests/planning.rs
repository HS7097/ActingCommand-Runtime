// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn planning_ipc_rejects_external_authority_fact_conflicts_without_poisoning_runtime() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("policy catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:authority-conflict",
        Vec::new(),
    ))
    .expect("authoritative fact");
    let mut facts = policy_facts();
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        fact_key: "env.ui_theme".to_owned(),
        value: FactValue::String("CallerValue".to_owned()),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        confidence_milli: 1_000,
    });

    let mut client = TestClient::connect(&host);
    let request = forward_projection_request(
        &facts,
        ForwardProjectionConfig::for_hours(1, 4).expect("projection config"),
    );
    let request = client.agent_request(RuntimeOperation::ProjectPolicyForward {
        request: Box::new(request),
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert!(matches!(
        receipt.error_projection(),
        Some(error) if error.code == RuntimeErrorCode::InvalidRequest && !error.fatal
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    let status = client.request(RuntimeOperation::Status);
    assert_eq!(client.send(&status).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn planning_ipc_rejects_oversized_forward_projection_without_poisoning_runtime() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&high_volume_forward_policy_sources(1))
        .expect("high-volume policy catalog");
    let config = ForwardProjectionConfig::next_24_hours();
    let projection = host
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            config,
        )
        .expect("bounded forward projection");
    assert_eq!(projection.steps.len(), 4_096);
    assert!(
        serde_json::to_vec(&projection)
            .expect("projection bytes")
            .len()
            > MAX_RUNTIME_PLANNING_DOCUMENT_BYTES
    );

    let mut client = TestClient::connect(&host);
    let request = forward_projection_request(&policy_facts(), config);
    let request = client.agent_request(RuntimeOperation::ProjectPolicyForward {
        request: Box::new(request),
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert!(matches!(
        receipt.error_projection(),
        Some(error) if error.code == RuntimeErrorCode::InvalidRequest && !error.fatal
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    let status = client.request(RuntimeOperation::Status);
    assert_eq!(client.send(&status).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_planning_overrun_does_not_publish_report_or_poison_runtime() {
    let root = TempDir::new().expect("tempdir");
    let aliases = (0..29)
        .map(|index| format!("fixture-instance-{index:02}"))
        .collect::<Vec<_>>();
    let mut input_facts = policy_facts();
    input_facts.outcomes.clear();
    input_facts.instances = aliases
        .iter()
        .map(|alias| InstanceSnapshot {
            instance_id: alias.clone(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        })
        .collect();
    let large_yield_points = (0..128)
        .map(|index| format!("checkpoint.{index:03}.{}", "x".repeat(108)))
        .collect::<Vec<_>>();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                input_facts.clone(),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                b"fixture procedure observe package v1",
                large_yield_points,
            )),
        Arc::new(FakeProvider::from_entries(
            aliases
                .iter()
                .map(|alias| (alias.clone(), instance_id(), Arc::clone(&state))),
        )),
    )
    .expect("runtime host");
    let sources = large_strategy_policy_sources(1);
    let compiled = actingcommand_policy::compile_catalog(&sources).expect("compiled catalog");
    let base = host
        .activate_policy_catalog(&sources)
        .expect("large strategy catalog");
    let evidence = host
        .store_test_report(b"synthetic oversized strategy evidence")
        .expect("strategy evidence");
    let resources = policy_resources();
    let (ledger_position, fact_snapshot_id) =
        strategic_frozen_identity(&host, &input_facts, &resources);
    let mut facts = input_facts;
    facts.ledger_position = ledger_position;
    facts.fact_snapshot_id = fact_snapshot_id;
    let assessments = aliases
        .iter()
        .map(|alias| StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: alias.clone(),
            game_id: "fixture-game-a".to_owned(),
            fact_snapshot_id: facts.fact_snapshot_id.clone(),
            current_projection: Some(10),
            production_rate_per_hour: Some(50),
            target: 100,
            deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
            available: true,
            capability_ids: vec!["operation.observe".to_owned()],
        })
        .collect();
    let report = strategy_report_with_assessments(
        &base,
        &evidence,
        facts.ledger_position,
        &facts.fact_snapshot_id,
        assessments,
    );
    let report_document =
        RuntimePlanningDocument::encode(RuntimePlanningDocumentKind::StrategicReport, &report)
            .expect("bounded strategic report input");
    let projection =
        actingcommand_policy::project_strategic_report(&compiled, &report, &facts, &resources)
            .expect("derived strategic projection");
    assert!(
        serde_json::to_vec(&projection)
            .expect("strategic projection bytes")
            .len()
            > MAX_RUNTIME_PLANNING_DOCUMENT_BYTES
    );

    let mut client = TestClient::connect(&host);
    let request = RuntimeStrategicReportRequest::new(report_document, vec![evidence])
        .expect("strategic report request");
    let request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(request),
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert!(matches!(
        receipt.error_projection(),
        Some(error) if error.code == RuntimeErrorCode::InvalidRequest && !error.fatal
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            }
        )
        .iter()
        .flat_map(|event| event.artifacts.iter())
        .all(|artifact| artifact.kind() != ArtifactKind::StrategyReport)
    );
    let status = client.request(RuntimeOperation::Status);
    assert_eq!(client.send(&status).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn planning_ipc_rejects_invalid_typed_documents_without_poisoning_runtime() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let evidence = host
        .store_test_report(b"synthetic planning evidence")
        .expect("planning evidence");
    let report = RuntimePlanningDocument::encode(
        RuntimePlanningDocumentKind::StrategicReport,
        &serde_json::json!({"not": "a strategic report"}),
    )
    .expect("typed planning envelope");
    let mut client = TestClient::connect(&host);
    let request = RuntimeStrategicReportRequest::new(report, vec![evidence])
        .expect("strategic report request");
    let request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(request),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close runtime");
}
