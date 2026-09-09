// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn project_interface_projects_runtime_domains_and_rejects_unknown_versions() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.current",
        ContractFactValue::Integer(5),
        "snapshot:project-interface",
        Vec::new(),
    ))
    .expect("publish fact");
    let (_, intent, reason_chain) = evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(
        &intent,
        &reason_chain,
        &PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: host.runtime_info().owner_epoch(),
            now_unix_ms: POLICY_NOW_UNIX_MS,
        },
    )
    .expect("admit dispatch");

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::current(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProjectInterface { response } = receipt.result().expect("result") else {
        panic!("expected project interface response");
    };
    let snapshot = response.snapshot().expect("current project snapshot");
    assert_eq!(
        snapshot.project.as_ref().expect("project").project_id,
        "fixture.catalog-a"
    );
    assert_eq!(snapshot.catalog.as_ref().expect("catalog").goal_count, 1);
    let current = response.current().expect("current runtime view");
    assert_eq!(current.instances.len(), 1);
    let current_source = current.source.as_ref().expect("committed current source");
    assert_eq!(current_source.sequence, current.observed_ledger_position);
    assert!(current_source.sequence > snapshot.ledger_position);
    let observations = projected_events(
        &mut client,
        EventQuery {
            request_id: Some(request.request_id()),
            event_type: Some(EventType::CommandValidated),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        observations.len(),
        1,
        "one state observation for the explicit request"
    );
    assert_eq!(observations[0].event_id, current_source.event_id);
    let ProjectionPayload::Full(payload) = &observations[0].payload else {
        panic!("current source payload");
    };
    assert!(
        matches!(payload.runtime_state(), Some(actingcommand_contract::RuntimeStateFact::Observed {
        state: actingcommand_contract::RuntimeObservedState::ProjectCurrent { status, fatal }, ..
    }) if status.owner_epoch() == current.owner_epoch && *fatal == current.fatal)
    );
    assert_eq!(snapshot.facts.len(), 1);
    assert_eq!(snapshot.goals.len(), 1);
    assert_eq!(snapshot.decisions.len(), 1);
    assert_eq!(snapshot.decisions[0].state, ProjectDecisionState::Admitted);
    let decision_page = &snapshot.decision_page;
    assert_eq!(decision_page.returned_count(), 1);
    assert!(!decision_page.has_more());
    assert_eq!(snapshot.approvals.len(), 1);
    assert!(!current.fatal);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);

    let unsupported = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::new(vec![
            "actingcommand.project-interface.v9".to_owned(),
        ])
        .expect("well-formed version request"),
    });
    let rejected = client.send(&unsupported);
    assert_eq!(rejected.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        rejected.error_projection().expect("typed rejection").code,
        RuntimeErrorCode::ProtocolInvalid
    );
}

#[test]
fn project_interface_v1_rejects_decision_history_that_requires_pagination() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");

    for index in 0..=actingcommand_contract::DEFAULT_PROJECT_DECISION_PAGE_SIZE {
        let (_, intent, reason_chain) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Reconciliation,
            POLICY_NOW_UNIX_MS + u64::from(index) * 60_000,
            10_000 + u64::from(index),
        );
        assert_eq!(
            host.admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
                .expect_err("unapproved decision")
                .code(),
            "policy_approval_fact_missing"
        );
    }

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::new(vec![
            actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V1.to_owned(),
        ])
        .expect("v1 request"),
    });
    let denied = client.send(&request);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("typed denial").code,
        RuntimeErrorCode::ProtocolInvalid
    );
    let request = ProjectInterfaceRequest::new(vec![
        actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V2.to_owned(),
    ])
    .expect("v2 request")
    .with_decision_page(ProjectDecisionPageRequest::new(2, None).expect("v2 page request"))
    .expect("paged v2 request");
    let request = client.request(RuntimeOperation::ProjectInterface { request });
    let denied = client.send(&request);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("typed denial").code,
        RuntimeErrorCode::ProtocolInvalid
    );
    host.close().expect("close host");
}

#[test]
fn project_interface_pages_decision_history_without_duplicates_or_loss() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.current",
        ContractFactValue::Integer(5),
        "snapshot:project-page-initial",
        Vec::new(),
    ))
    .expect("publish initial project fact");

    let (_, approval_basis, _) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::FactsChanged, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &approval_basis);
    let (_, admitted, admitted_reason) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        101,
    );
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted,
            &admitted_reason,
            &policy_context(&host, &admitted),
        )
        .expect("approved dispatch"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    record_policy_approval_disposition(&host, &admitted, ApprovalDisposition::Revoked);

    let mut expected = vec![admitted.decision_id.clone()];
    let mut late_approval_intent = None;
    for index in 0..4_u64 {
        let (_, intent, reason_chain) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Reconciliation,
            POLICY_NOW_UNIX_MS + (index + 2) * 60_000,
            102 + index,
        );
        late_approval_intent.get_or_insert_with(|| intent.clone());
        expected.push(intent.decision_id.clone());
        assert_eq!(
            host.admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
                .expect_err("unapproved dispatch must be rejected")
                .code(),
            "policy_approval_fact_missing"
        );
    }
    expected.reverse();

    let mut client = TestClient::connect(&host);
    let mut cursor = None;
    let mut collected = Vec::new();
    let mut collected_states = BTreeMap::new();
    let mut frozen_projection = None;
    let mut queued_waiter = None;
    for page_index in 0..4 {
        let request = ProjectInterfaceRequest::current()
            .with_decision_page(
                ProjectDecisionPageRequest::new(2, cursor.clone()).expect("page request"),
            )
            .expect("paged project request");
        let request = client.request(RuntimeOperation::ProjectInterface { request });
        let receipt = client.send(&request);
        let RuntimeResult::ProjectInterface { response } = receipt.result().expect("page result")
        else {
            panic!("expected project interface response")
        };
        let snapshot = response.snapshot().expect("current project snapshot");
        let current = response.current().expect("current runtime view");
        assert!(current.observed_ledger_position >= snapshot.ledger_position);
        let source = current.source.as_ref().expect("paged current source");
        assert_eq!(source.sequence, current.observed_ledger_position);
        assert!(source.sequence > snapshot.ledger_position);
        let current_instance = current.instances.first().expect("project instance");
        if page_index == 0 {
            assert!(current_instance.lease_active);
            assert_eq!(current_instance.queued_request_count, 0);
        } else {
            assert!(current_instance.lease_active);
            assert_eq!(current_instance.queued_request_count, 1);
        }
        let projection = (
            snapshot.ledger_position,
            snapshot.catalog.clone(),
            snapshot.facts.clone(),
            snapshot.approvals.clone(),
        );
        if let Some(frozen) = &frozen_projection {
            assert_eq!(
                &projection, frozen,
                "every page must retain the first page ledger projection"
            );
        } else {
            frozen_projection = Some(projection);
        }
        assert!(snapshot.decisions.len() <= 2);
        for decision in &snapshot.decisions {
            collected.push(decision.decision_id.clone());
            collected_states.insert(decision.decision_id.clone(), decision.state);
        }
        let page = &snapshot.decision_page;
        assert_eq!(usize::from(page.returned_count()), snapshot.decisions.len());
        cursor = page.next_cursor().cloned();
        if page_index == 0 {
            let mut waiter = TestClient::connect(&host);
            let queued = waiter.request(RuntimeOperation::queue_lease(
                POLICY_INSTANCE_ALIAS,
                waiter.ids.mint_holder_id().expect("waiter holder"),
                LeaseQueuePolicy::new(LeasePriority::Normal, 60_000).expect("queue policy"),
            ));
            assert!(matches!(
                waiter.send(&queued).result(),
                Some(RuntimeResult::LeaseQueued { .. })
            ));
            queued_waiter = Some(waiter);
            host.complete_policy_dispatch(&admitted.decision_id)
                .expect("complete after snapshot");
            let mut late_fact = stored_fact(
                FactScope::Instance {
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                },
                "resource.current",
                ContractFactValue::Integer(9),
                "snapshot:project-page-late",
                Vec::new(),
            );
            late_fact.observed_at_unix_ms += 1;
            host.publish_fact(late_fact)
                .expect("publish fact after first page");
            record_policy_approval(
                &host,
                late_approval_intent.as_ref().expect("late approval intent"),
            );
            host.activate_policy_catalog(&budget_policy_sources(2))
                .expect("activate catalog after first page");
        }
        if !page.has_more() {
            break;
        }
    }

    assert_eq!(collected, expected);
    assert_eq!(
        collected_states.get(&admitted.decision_id),
        Some(&ProjectDecisionState::Admitted),
        "later pages must remain bound to the first page ledger snapshot"
    );
    assert!(cursor.is_none());
    let current = project_snapshot(&host, ProjectInterfaceRequest::current());
    assert_eq!(
        current
            .catalog
            .as_ref()
            .expect("current catalog")
            .catalog_version,
        2
    );
    assert!(
        current
            .facts
            .iter()
            .any(|fact| fact.source_snapshot_id == "snapshot:project-page-late")
    );
    assert_ne!(
        &current.approvals,
        &frozen_projection.as_ref().expect("frozen projection").3
    );
    drop(queued_waiter);
    drop(client);
    host.close().expect("close host");
}
