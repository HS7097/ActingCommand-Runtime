// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn approval_decision_is_authoritative_target_bound_and_revocable() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    let forged = policy_context(&host, &intent);
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &forged)
            .expect_err("caller approval set is not authoritative")
            .code(),
        "policy_approval_fact_missing"
    );

    record_policy_approval(&host, &intent);
    let (_, approved_intent, approved_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        8,
    );
    assert!(matches!(
        host.admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&host, &approved_intent),
        )
        .expect("approved dispatch"),
        PolicyDispatchAdmission::Granted { .. }
    ));

    let mut client = TestClient::connect(&host);
    client.authenticate_governance();
    let conflicting = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:fixture-a",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "f".repeat(64)),
                catalog_version: 1,
            },
            "user_confirmed",
        )
        .expect("conflicting approval"),
    });
    assert_eq!(
        client.send(&conflicting).state(),
        RuntimeReceiptState::Denied
    );
    drop(client);

    host.complete_policy_dispatch(&approved_intent.decision_id)
        .expect("complete approved dispatch");
    record_policy_approval_disposition(&host, &approved_intent, ApprovalDisposition::Revoked);
    let (_, after_revoke, after_revoke_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 120_000,
        9,
    );
    assert_eq!(
        host.admit_policy_dispatch(
            &after_revoke,
            &after_revoke_reasons,
            &policy_context(&host, &after_revoke),
        )
        .expect_err("revoked approval must not authorize a new dispatch")
        .code(),
        "policy_approval_fact_missing"
    );

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 2);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn approval_history_compacts_without_losing_durable_target_identity() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let target = ApprovalTarget::Catalog {
        catalog_hash: format!("sha256:{}", "a".repeat(64)),
        catalog_version: 1,
    };
    let mut client = TestClient::connect(&host);
    client.authenticate_governance();
    for index in 0..257 {
        let approval_id = format!("approval:history-{index}");
        let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
            decision: ApprovalDecisionRecord::new(
                approval_id,
                ApprovalDisposition::Rejected,
                target.clone(),
                "history_compaction",
            )
            .expect("approval decision"),
        });
        assert_eq!(
            client.send(&request).state(),
            RuntimeReceiptState::Completed
        );
    }
    drop(client);

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::current(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProjectInterface { response } = receipt.result().expect("projection result")
    else {
        panic!("expected project interface response")
    };
    assert_eq!(
        response
            .snapshot()
            .expect("current project snapshot")
            .approvals
            .len(),
        256
    );
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    let mut client = TestClient::connect(&reopened);
    client.authenticate_governance();
    let conflict = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:history-0",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "b".repeat(64)),
                catalog_version: 1,
            },
            "conflicting_target",
        )
        .expect("conflicting approval"),
    });
    assert_eq!(client.send(&conflict).state(), RuntimeReceiptState::Denied);
    assert!(reopened.fatal_error().expect("runtime health").is_none());
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn governance_authority_is_capability_authenticated_and_connection_bound() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let target = ApprovalTarget::Catalog {
        catalog_hash: format!("sha256:{}", "a".repeat(64)),
        catalog_version: 1,
    };
    let approval = |disposition| {
        ApprovalDecisionRecord::new(
            "approval:governance-boundary",
            disposition,
            target.clone(),
            "user_confirmed",
        )
        .expect("approval decision")
    };

    let mut client = TestClient::connect(&host);
    let unauthenticated = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Approved),
    });
    let receipt = client.send(&unauthenticated);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    let wrong_capability = client.governance_request(RuntimeOperation::AuthenticateGovernance {
        capability: "wrong-governance-capability-value".to_owned(),
    });
    let receipt = client.send(&wrong_capability);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    client.authenticate_governance();
    let approved = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Approved),
    });
    assert_eq!(
        client.send(&approved).state(),
        RuntimeReceiptState::Completed
    );

    let mut other = TestClient::connect(&host);
    let forged_revocation = other.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Revoked),
    });
    let receipt = other.send(&forged_revocation);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    let revoked = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Revoked),
    });
    assert_eq!(
        client.send(&revoked).state(),
        RuntimeReceiptState::Completed
    );

    let events = projected_events(
        &mut other,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| {
        event.origin.source() == EventSource::Ui
            && event.origin.module() == OriginModule::Governance
            && event.origin.actor() == EventActor::User
    }));
    drop(client);
    drop(other);
    host.close().expect("close host");
}

#[test]
fn historical_agent_approval_poisoning_is_fatal_during_recovery() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.append_approval_event_for_test(
        EventSource::Adapter,
        EventActor::Agent,
        ApprovalDecisionRecord::new(
            "approval:historical-agent",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "a".repeat(64)),
                catalog_version: 1,
            },
            "agent_claimed_approval",
        )
        .expect("approval decision"),
    )
    .expect("historical malicious approval fixture");
    host.close().expect("close host");

    let error = match RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    ) {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("historical Agent approval must prevent recovery")
        }
        Err(error) => error,
    };
    assert!(error.is_fatal());
    assert_eq!(error.code(), "approval_projection_origin_invalid");
}
