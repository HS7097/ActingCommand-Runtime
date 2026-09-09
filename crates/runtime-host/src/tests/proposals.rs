// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn proposal_a_b_c_pipeline_requires_reports_and_authoritative_approvals() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("base catalog");
    let report = host
        .store_test_report(b"synthetic immutable strategy report")
        .expect("strategy report");
    let mut client = TestClient::connect(&host);

    let proposal_a = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![report.clone()],
        ProposalKind::ParameterInstantiation {
            instantiation: TaskTemplateInstantiation::new(
                "fixture.observe",
                "fixture.observe-copy",
                POLICY_INSTANCE_ALIAS,
                Some(110),
                Some(1_100),
            )
            .expect("template instantiation"),
        },
    )
    .expect("class A proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalEvaluated { preview: preview_a } =
        receipt.result().expect("proposal preview")
    else {
        panic!("expected proposal preview")
    };
    assert_eq!(preview_a.class(), ProposalClass::A);
    assert_eq!(
        preview_a.disposition(),
        ProposalDisposition::ReadyForApproval
    );

    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-plan-a",
        preview_a.approval_target().expect("plan target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-template-a",
        ApprovalTarget::Catalog {
            catalog_hash: base.catalog_hash().to_owned(),
            catalog_version: base.catalog_version(),
        },
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalPromoted {
        promotion: promotion_a,
    } = receipt.result().expect("proposal promotion")
    else {
        panic!("expected proposal promotion")
    };
    assert_eq!(promotion_a.preview().class(), ProposalClass::A);
    assert_eq!(promotion_a.approval_fact_ids().len(), 2);
    let activated = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogActivated),
            ..EventQuery::default()
        },
    );
    let authorization = activated
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Catalog(CatalogPayload::Activated(payload)) => payload.promotion(),
                _ => None,
            },
            _ => None,
        })
        .expect("durable proposal authorization");
    assert_eq!(authorization.proposal_id(), proposal_a.proposal_id());
    assert_eq!(authorization.class(), ProposalClass::A);
    assert_eq!(
        authorization.approval_fact_ids(),
        promotion_a.approval_fact_ids()
    );
    assert_eq!(authorization.report_artifact_ids(), [report.artifact_id]);
    let active_a = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    assert_eq!(active_a.catalog_version(), 2);
    assert_eq!(
        active_a.catalog_hash(),
        preview_a.target_catalog_hash().expect("target hash")
    );
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::CatalogChanged,
        )
        .expect("evaluate promoted catalog");
    assert!(
        cycle
            .evaluation
            .expect("policy evaluation")
            .decisions
            .iter()
            .any(|decision| decision.task_id == "fixture.observe-copy")
    );
    let activated_before_replay = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogActivated),
            ..EventQuery::default()
        },
    )
    .len();
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::CatalogActivated),
                ..EventQuery::default()
            },
        )
        .len(),
        activated_before_replay
    );

    let mut patches = proposal_version_patches(3);
    patches.push(
        CatalogDeclarationPatch::new(
            ProposalDocument::Tasks,
            ProposalPatchOperation::Replace,
            "/tasks/0/priority",
            Some("111".to_owned()),
        )
        .expect("priority patch"),
    );
    let proposal_b = CatalogProposal::new(
        active_a.catalog_hash(),
        active_a.catalog_version(),
        3,
        vec![report.clone()],
        ProposalKind::CatalogDiff { patches },
    )
    .expect("class B proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_b.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalEvaluated { preview: preview_b } =
        receipt.result().expect("class B preview")
    else {
        panic!("expected class B preview")
    };
    assert_eq!(preview_b.class(), ProposalClass::B);
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_b.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-plan-b",
        preview_b.approval_target().expect("class B plan target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_b),
    });
    let receipt = client.send(&request);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ProposalPromoted { promotion })
            if promotion.preview().class() == ProposalClass::B
                && promotion.approval_fact_ids() == ["approval:proposal-plan-b"]
    ));
    let active_b = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    assert_eq!(active_b.catalog_version(), 3);

    let proposal_c = CatalogProposal::new(
        active_b.catalog_hash(),
        active_b.catalog_version(),
        4,
        vec![report],
        ProposalKind::LanguageExtension {
            extension_code: "predicate.new-observation".to_owned(),
        },
    )
    .expect("class C proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_c.clone()),
    });
    let receipt = client.send(&request);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ProposalEvaluated { preview })
            if preview.class() == ProposalClass::C
                && preview.disposition() == ProposalDisposition::NeedsHumanSpecification
                && preview.approval_target().is_none()
    ));
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_c),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_version(),
        3
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn proposal_rejects_unverified_reports_and_invalid_packs_without_partial_activation() {
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
    let base = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("base catalog");
    let report = host
        .store_test_report(b"synthetic immutable strategy report")
        .expect("strategy report");
    let mut client = TestClient::connect(&host);

    let forged = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![unverified_report(&report, &client.ids)],
        ProposalKind::ParameterInstantiation {
            instantiation: TaskTemplateInstantiation::new(
                "fixture.observe",
                "fixture.unverified",
                POLICY_INSTANCE_ALIAS,
                None,
                None,
            )
            .expect("template instantiation"),
        },
    )
    .expect("forged proposal shape");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(forged),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert!(host.fatal_error().expect("runtime health").is_none());

    let mut patches = proposal_version_patches(2);
    patches.push(
        CatalogDeclarationPatch::new(
            ProposalDocument::Tasks,
            ProposalPatchOperation::Replace,
            "/tasks/0/priority",
            Some("\"not-an-integer\"".to_owned()),
        )
        .expect("invalid typed priority patch"),
    );
    let invalid_pack = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![report],
        ProposalKind::CatalogDiff { patches },
    )
    .expect("invalid compiled proposal shape");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(invalid_pack),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    assert_eq!(
        fs::read_dir(root.path().join("policy/catalogs/generations"))
            .expect("catalog generations")
            .count(),
        1
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::CatalogActivated),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close runtime");
}
