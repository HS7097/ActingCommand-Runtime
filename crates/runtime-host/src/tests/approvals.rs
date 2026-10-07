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
    client.declare_governance_identity();
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
    let latest = events.last().expect("latest approval fact");
    drop(client);
    host.close().expect("close host");
    let database =
        actingcommand_runtime_database::RuntimeDatabase::open_existing(root.path(), true)
            .expect("existing approval database");
    let state = actingcommand_runtime_state::RuntimeStateStore::from_database(Arc::new(database))
        .expect("verified State projection");
    let entry = state
        .read_projection_entry(
            actingcommand_runtime_state::APPROVAL_PROJECTION_NAMESPACE,
            &format!("{:x}", Sha256::digest(b"approval:fixture-a")),
        )
        .expect("integrity-protected approval projection")
        .expect("latest projection");
    assert_eq!(entry.ledger_sequence(), latest.sequence);
    let projected: ApprovalDecisionRecord =
        serde_json::from_slice(entry.payload()).expect("typed approval projection");
    assert_eq!(projected.disposition(), ApprovalDisposition::Revoked);
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
    client.declare_governance_identity();
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

    // Rebuild a missing derived projection from the full retained approval history.
    let database =
        actingcommand_runtime_database::RuntimeDatabase::open_existing(root.path(), false)
            .expect("existing approval database");
    database
        .connection("remove_derived_approval_projection")
        .expect("database connection")
        .execute(
            "DELETE FROM projection_entries WHERE namespace = 'approval.latest.v1'",
            [],
        )
        .expect("remove only derived approval rows");
    drop(database);

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
    client.declare_governance_identity();
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
fn governance_authority_requires_an_accepted_identity_card_and_is_connection_bound() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_governance_policy(test_governance_policy()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host with a governance allow-list");
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

    let not_allowed = client.governance_request(RuntimeOperation::DeclareGovernanceIdentity {
        card: actingcommand_contract::GovernanceIdentityCard {
            client: "not-allowed-client".to_owned(),
            ..test_governance_card()
        },
    });
    let receipt = client.send(&not_allowed);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    let denial = receipt.error_projection().expect("denial");
    assert_eq!(denial.code, RuntimeErrorCode::InvalidRequest);
    assert_eq!(denial.host_code(), Some("governance_client_not_allowed"));
    assert!(receipt.terminal().is_some(), "the refusal is recorded");

    client.declare_governance_identity();
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
    let declarations = projected_events(
        &mut other,
        EventQuery {
            event_type: Some(EventType::GovernanceIdentityDeclared),
            ..EventQuery::default()
        },
    );
    let verdicts = declarations
        .iter()
        .map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Client(
                    actingcommand_contract::ClientPayload::GovernanceIdentityDeclared(declared),
                ) => declared.verdict(),
                other => panic!("unexpected declaration payload {other:?}"),
            },
            other => panic!("unexpected declaration projection {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts,
        [
            actingcommand_contract::GovernanceIdentityVerdict::Refused {
                code: actingcommand_contract::GovernanceIdentityRefusal::ClientNotAllowed,
            },
            actingcommand_contract::GovernanceIdentityVerdict::Accepted,
        ]
    );
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

/// Workflow #361 A: a catalog with another id, switched to with `replace`, a rollback to the
/// first lineage and a second rollback to the other one. The planned approvals follow the
/// lineage (the other lineage's approvals are revoked, a restored generation's configured
/// approval is approved again), an admission pinned to the first generation still settles
/// after the switch, and the ledger replays to the same generation after a restart. A
/// revocation by a person is never replaced.
#[test]
fn catalog_lineage_switch_and_rollbacks_replay_with_reapprovals() {
    fn other_lineage(mut sources: CatalogSources) -> CatalogSources {
        for source in [
            &mut sources.tasks,
            &mut sources.pools,
            &mut sources.activity,
            &mut sources.timeline,
        ] {
            let mut document: serde_json::Value =
                serde_json::from_slice(&source.bytes).expect("lineage fixture JSON");
            document["catalog"]["catalog_id"] = serde_json::json!("fixture.catalog-b");
            document["catalog"]["approval_refs"] = serde_json::json!(["approval:fixture-b"]);
            source.bytes = serde_json::to_vec_pretty(&document).expect("lineage fixture bytes");
        }
        sources
    }
    fn record_planned(host: &RuntimeHost, plan: &CatalogTransitionPlan) {
        let mut client = TestClient::connect(host);
        client.declare_governance_identity();
        let approvals = plan.approvals();
        for decision in approvals
            .revoke()
            .iter()
            .chain(approvals.record())
            .chain(approvals.reapprove())
        {
            let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
                decision: decision.clone(),
            });
            assert_eq!(
                client.send(&request).state(),
                RuntimeReceiptState::Completed
            );
        }
    }
    fn ids(records: &[ApprovalDecisionRecord]) -> Vec<&str> {
        records
            .iter()
            .map(ApprovalDecisionRecord::approval_id)
            .collect()
    }
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let start = |root: &TempDir| {
        RuntimeHost::start(
            config(root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host")
    };
    let lineage_a = budget_policy_sources(1);
    let lineage_b = other_lineage(budget_policy_sources(1));
    let ids_a = vec!["approval:fixture-a".to_owned()];
    let ids_b = vec!["approval:fixture-b".to_owned()];
    let host = start(&root);

    let first = host
        .plan_policy_catalog_transition(&lineage_a, None, &ids_a)
        .expect("first plan");
    assert_eq!(first.kind(), CatalogTransitionPlanKind::First);
    let generation_a = host
        .apply_policy_catalog_transition(&first)
        .expect("first activation");
    record_planned(&host, &first);
    assert_eq!(ids(first.approvals().record()), ["approval:fixture-a"]);

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert!(matches!(
        host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .expect("admission on the first lineage"),
        PolicyDispatchAdmission::Granted { .. }
    ));

    assert_eq!(
        host.plan_policy_catalog_transition(&lineage_b, None, &ids_b)
            .expect_err("another id needs replace")
            .code(),
        "catalog_activation_not_newer"
    );
    let replace_a = CatalogTransitionRequest::replace(generation_a.catalog_hash());
    let switched = host
        .plan_policy_catalog_transition(&lineage_b, Some(&replace_a), &ids_b)
        .expect("switch plan");
    assert_eq!(switched.kind(), CatalogTransitionPlanKind::Switch);
    assert_eq!(ids(switched.approvals().revoke()), ["approval:fixture-a"]);
    assert_eq!(ids(switched.approvals().record()), ["approval:fixture-b"]);
    let generation_b = host
        .apply_policy_catalog_transition(&switched)
        .expect("switch");
    assert_eq!(generation_b.catalog_id(), "fixture.catalog-b");
    record_planned(&host, &switched);
    assert_eq!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        generation_a.catalog_hash()
    );
    host.complete_policy_dispatch(&intent.decision_id)
        .expect("the admission pinned to the first lineage settles after the switch");

    let replace_b = CatalogTransitionRequest::replace(generation_b.catalog_hash());
    assert_eq!(
        host.plan_policy_catalog_transition(&lineage_a, Some(&replace_a), &ids_a)
            .expect_err("a stale expectation is refused")
            .code(),
        "catalog_transition_expectation_mismatch"
    );
    let rolled_back = host
        .plan_policy_catalog_transition(&lineage_a, Some(&replace_b), &ids_a)
        .expect("rollback plan");
    assert_eq!(rolled_back.kind(), CatalogTransitionPlanKind::Rollback);
    assert_eq!(
        ids(rolled_back.approvals().revoke()),
        ["approval:fixture-b"]
    );
    assert!(rolled_back.approvals().record().is_empty());
    assert_eq!(
        ids(rolled_back.approvals().reapprove()),
        ["approval:fixture-a"]
    );
    host.apply_policy_catalog_transition(&rolled_back)
        .expect("rollback");
    record_planned(&host, &rolled_back);

    let forward_again = host
        .plan_policy_catalog_transition(&lineage_b, Some(&replace_a), &ids_b)
        .expect("second rollback plan");
    assert_eq!(forward_again.kind(), CatalogTransitionPlanKind::Rollback);
    assert_eq!(
        ids(forward_again.approvals().reapprove()),
        ["approval:fixture-b"]
    );
    host.apply_policy_catalog_transition(&forward_again)
        .expect("second rollback");
    record_planned(&host, &forward_again);
    let mut client = TestClient::connect(&host);
    let transitions = projected_events(&mut client, EventQuery::default())
        .into_iter()
        .filter_map(|event| match event.event_type {
            EventType::CatalogActivated | EventType::CatalogRolledBack => Some(event.event_type),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transitions,
        [
            EventType::CatalogActivated,
            EventType::CatalogActivated,
            EventType::CatalogRolledBack,
            EventType::CatalogRolledBack,
        ]
    );
    drop(client);
    host.close().expect("close host");

    let reopened = start(&root);
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog"),
        generation_b
    );
    let unchanged = reopened
        .plan_policy_catalog_transition(&lineage_b, Some(&replace_a), &ids_b)
        .expect("unchanged plan after restart");
    assert_eq!(unchanged.kind(), CatalogTransitionPlanKind::Unchanged);
    assert_eq!(unchanged.approvals(), &CatalogApprovalPlan::default());

    let mut client = TestClient::connect(&reopened);
    client.declare_governance_identity();
    let revocation = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:fixture-b",
            ApprovalDisposition::Revoked,
            ApprovalTarget::Catalog {
                catalog_hash: generation_b.catalog_hash().to_owned(),
                catalog_version: generation_b.catalog_version(),
            },
            "user_revoked",
        )
        .expect("revocation by a person"),
    });
    assert_eq!(
        client.send(&revocation).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    assert_eq!(
        reopened
            .plan_policy_catalog_transition(&lineage_b, None, &ids_b)
            .expect_err("a revocation by a person is never replaced")
            .code(),
        "policy_catalog_approval_conflict"
    );
    reopened.close().expect("close reopened host");
}

/// Workflow #361 A (review M1): a rollback within one catalog id keeps the later version's
/// approval, so re-applying the identical later generation with an unchanged configuration is
/// an ordinary forward step that records nothing new for it.
#[test]
fn catalog_rollback_then_forward_again_keeps_the_later_approval() {
    fn record_planned(host: &RuntimeHost, plan: &CatalogTransitionPlan) {
        let mut client = TestClient::connect(host);
        client.declare_governance_identity();
        let approvals = plan.approvals();
        for decision in approvals
            .revoke()
            .iter()
            .chain(approvals.record())
            .chain(approvals.reapprove())
        {
            let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
                decision: decision.clone(),
            });
            assert_eq!(
                client.send(&request).state(),
                RuntimeReceiptState::Completed
            );
        }
    }
    fn ids(records: &[ApprovalDecisionRecord]) -> Vec<&str> {
        records
            .iter()
            .map(ApprovalDecisionRecord::approval_id)
            .collect()
    }
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let version_1 = budget_policy_sources(1);
    let version_2 = budget_policy_sources(2);
    let ids_1 = vec!["approval:fixture-a".to_owned()];
    let ids_2 = vec!["approval:fixture-a2".to_owned()];
    let drive = |sources: &CatalogSources,
                 request: Option<&CatalogTransitionRequest>,
                 approval_ids: &[String]| {
        let plan = host
            .plan_policy_catalog_transition(sources, request, approval_ids)
            .expect("transition plan");
        host.apply_policy_catalog_transition(&plan)
            .expect("planned transition");
        record_planned(&host, &plan);
        plan
    };
    let first = drive(&version_1, None, &ids_1);
    assert_eq!(first.kind(), CatalogTransitionPlanKind::First);
    let forward = drive(&version_2, None, &ids_2);
    assert_eq!(forward.kind(), CatalogTransitionPlanKind::Forward);
    assert_eq!(ids(forward.approvals().revoke()), ["approval:fixture-a"]);
    assert_eq!(ids(forward.approvals().record()), ["approval:fixture-a2"]);
    let replace_2 = CatalogTransitionRequest::replace(forward.generation().catalog_hash());
    let rollback = drive(&version_1, Some(&replace_2), &ids_1);
    assert_eq!(rollback.kind(), CatalogTransitionPlanKind::Rollback);
    assert!(rollback.approvals().revoke().is_empty());
    assert_eq!(
        ids(rollback.approvals().reapprove()),
        ["approval:fixture-a"]
    );
    let again = drive(&version_2, None, &ids_2);
    assert_eq!(again.kind(), CatalogTransitionPlanKind::Forward);
    assert!(again.approvals().record().is_empty());
    assert!(again.approvals().reapprove().is_empty());
    assert_eq!(ids(again.approvals().revoke()), ["approval:fixture-a"]);
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog"),
        *forward.generation()
    );
    host.close().expect("close host");
}
