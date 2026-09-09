// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn strategic_report_uses_authoritative_policy_projection() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic authoritative strategy evidence")
        .expect("strategy evidence");
    let mut client = TestClient::connect(&host);
    let publish = client.agent_request(RuntimeOperation::PublishFact {
        record: stored_fact(
            FactScope::Instance {
                instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            "resource.primary",
            ContractFactValue::Integer(60),
            "snapshot:authoritative-strategy",
            Vec::new(),
        ),
    });
    assert_eq!(
        client.send(&publish).state(),
        RuntimeReceiptState::Completed
    );
    let as_of_ledger_position =
        project_snapshot(&host, ProjectInterfaceRequest::current()).ledger_position;
    let identity_request = client.agent_request(RuntimeOperation::ProjectPolicyInputIdentity {
        as_of_ledger_position,
    });
    let identity_receipt = client.send(&identity_request);
    assert_eq!(identity_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::PolicyInputIdentityProjected { identity } = identity_receipt
        .result()
        .expect("policy input identity result")
    else {
        panic!("expected policy input identity result")
    };
    assert_eq!(identity.ledger_position(), as_of_ledger_position);
    let ledger_position = identity.ledger_position();
    let fact_snapshot_id = identity.fact_snapshot_id().to_owned();
    let artifact_id = serde_json::to_value(evidence.artifact_id)
        .expect("artifact id JSON")
        .as_str()
        .expect("artifact id string")
        .to_owned();
    let template = |template_id: &str| StrategicTemplate {
        template_id: template_id.to_owned(),
        task_template_ids: vec!["fixture.observe".to_owned()],
        activity_profile_template_id: "fixture-activity-game".to_owned(),
        eligibility: PredicateSpec::Fact {
            scope: ScopeSelector::Game {
                game_id: "fixture-game-a".to_owned(),
            },
            fact_key: "feature.enabled".to_owned(),
            comparison: Comparison::Eq,
            value: FactValue::Boolean(true),
            max_age_ms: Some(60_000),
        },
        match_bands: vec![
            StrategicBand::Actionable,
            StrategicBand::InfeasibleBestEffort,
        ],
        minimum_urgency_milli: 0,
        maximum_urgency_milli: 1_000_000,
        strategic_weight_milli: 500,
        load_profile: LoadProfile::Weighted {
            cpu_milli: 200,
            gpu_milli: 100,
            io_milli: 300,
        },
        risk_class: "standard".to_owned(),
        budget_class: "bounded".to_owned(),
    };
    let make_report = |fact_check| {
        StrategicReport::new(
            "fixture-game-a",
            base.catalog_hash(),
            base.catalog_version(),
            base.catalog_version() + 1,
            ledger_position,
            POLICY_NOW_UNIX_MS,
            format!("sha256:{}", "d".repeat(64)),
            format!("sha256:{}", "e".repeat(64)),
            vec![StrategicEvidencePointer {
                artifact_id: artifact_id.clone(),
                sha256: evidence.sha256.clone(),
            }],
            vec![
                StrategicGoal {
                    goal_id: "goal.fact".to_owned(),
                    goal_version: 1,
                    metric: MetricRef::Fact {
                        fact_key: "resource.primary".to_owned(),
                    },
                    templates: vec![template("template.fact")],
                    outlier_policy: OutlierPolicy {
                        metric: OutlierMetric::Shortfall,
                        mad_multiplier_milli: 2_000,
                        top_n: 1,
                    },
                },
                StrategicGoal {
                    goal_id: "goal.pool".to_owned(),
                    goal_version: 1,
                    metric: MetricRef::Pool {
                        pool_id: "fixture-pool-a".to_owned(),
                    },
                    templates: vec![template("template.pool")],
                    outlier_policy: OutlierPolicy {
                        metric: OutlierMetric::Shortfall,
                        mad_multiplier_milli: 2_000,
                        top_n: 1,
                    },
                },
            ],
            vec![
                StrategicInstanceAssessment {
                    goal_id: "goal.fact".to_owned(),
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                    game_id: "fixture-game-a".to_owned(),
                    fact_snapshot_id: fact_snapshot_id.clone(),
                    current_projection: fact_check,
                    production_rate_per_hour: None,
                    target: 50,
                    deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                    available: true,
                    capability_ids: vec!["operation.observe".to_owned()],
                },
                StrategicInstanceAssessment {
                    goal_id: "goal.pool".to_owned(),
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                    game_id: "fixture-game-a".to_owned(),
                    fact_snapshot_id: fact_snapshot_id.clone(),
                    current_projection: None,
                    production_rate_per_hour: None,
                    target: 70,
                    deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                    available: true,
                    capability_ids: vec!["operation.observe".to_owned()],
                },
            ],
            CohortBudgets {
                max_active: 2,
                max_prompt: 1,
            },
        )
        .expect("strategic report")
    };

    let mismatched = make_report(Some(61));
    let mismatched_document =
        RuntimePlanningDocument::encode(RuntimePlanningDocumentKind::StrategicReport, &mismatched)
            .expect("mismatched report document");
    let mismatched_request =
        RuntimeStrategicReportRequest::new(mismatched_document, vec![evidence.clone()])
            .expect("mismatched report request");
    let mismatched_request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(mismatched_request),
    });
    assert_eq!(
        client.send(&mismatched_request).state(),
        RuntimeReceiptState::Denied
    );

    let report = make_report(None);
    let document =
        RuntimePlanningDocument::encode(RuntimePlanningDocumentKind::StrategicReport, &report)
            .expect("strategy report document");
    let transport = RuntimeStrategicReportRequest::new(document, vec![evidence.clone()])
        .expect("strategy report request");
    let first_request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(transport.clone()),
    });
    let first_receipt = client.send(&first_request);
    assert_eq!(first_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::StrategicPlanPrepared { plan: first_plan } =
        first_receipt.result().expect("first strategic plan result")
    else {
        panic!("expected first strategic plan result")
    };
    let first_plan = first_plan.as_ref().clone();
    let second_request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(transport),
    });
    let second_receipt = client.send(&second_request);
    assert_eq!(second_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::StrategicPlanPrepared { plan: second_plan } = second_receipt
        .result()
        .expect("second strategic plan result")
    else {
        panic!("expected second strategic plan result")
    };
    assert_eq!(
        serde_json::to_vec(&first_plan).expect("first result bytes"),
        serde_json::to_vec(second_plan).expect("second result bytes")
    );
    let (_, projection, _, _) = first_plan.into_parts();
    let projection: actingcommand_policy::StrategicProjection = projection
        .decode(RuntimePlanningDocumentKind::StrategicProjection)
        .expect("strategic projection");
    let fact = projection
        .instances
        .iter()
        .find(|instance| instance.goal_id == "goal.fact")
        .expect("fact projection");
    assert_eq!(fact.band, StrategicBand::NoPressure);
    assert_eq!(fact.shortfall, Some(0));
    assert_eq!(fact.capacity, Some(0));
    assert_eq!(fact.urgency_milli, Some(0));
    let pool = projection
        .instances
        .iter()
        .find(|instance| instance.goal_id == "goal.pool")
        .expect("pool projection");
    assert_eq!(pool.band, StrategicBand::InfeasibleBestEffort);
    assert_eq!(pool.shortfall, Some(60));
    assert_eq!(pool.capacity, Some(50));
    assert_eq!(pool.urgency_milli, Some(1_200));
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    let signal_events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signal_events.len(), 1);
    let ProjectionPayload::Full(payload) = &signal_events[0].payload else {
        panic!("expected full planning signal payload")
    };
    let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal)) = payload.as_ref()
    else {
        panic!("expected planning signal payload")
    };
    assert_eq!(signal.kind(), PolicyPlanningSignalKind::FeasibilityRed);
    assert_eq!(signal.instance_id(), POLICY_INSTANCE_ALIAS);
    assert_eq!(signal.task_id(), None);
    assert_eq!(signal.fact_code(), "strategic.feasibility_red");
    assert_eq!(signal.observed_at_unix_ms(), POLICY_NOW_UNIX_MS);
    assert_eq!(signal.detection_budget(), None);
    assert!(signal.signal_id().starts_with("signal:strategic:"));
    assert_eq!(signal.signal_id().len(), 81);
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);

    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_report_records_timeline_reached_once() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic timeline strategy evidence")
        .expect("strategy evidence");
    let input_facts = policy_facts();
    let resources = policy_resources();
    let (ledger_position, fact_snapshot_id) =
        strategic_frozen_identity(&host, &input_facts, &resources);
    let report = strategy_report_with_assessments(
        &base,
        &evidence,
        ledger_position,
        &fact_snapshot_id,
        vec![StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            game_id: "fixture-game-a".to_owned(),
            fact_snapshot_id: fact_snapshot_id.clone(),
            current_projection: Some(10),
            production_rate_per_hour: Some(50),
            target: 100,
            deadline_unix_ms: POLICY_NOW_UNIX_MS,
            available: true,
            capability_ids: vec!["operation.observe".to_owned()],
        }],
    );
    let document =
        RuntimePlanningDocument::encode(RuntimePlanningDocumentKind::StrategicReport, &report)
            .expect("strategy report document");
    let transport = RuntimeStrategicReportRequest::new(document, vec![evidence])
        .expect("strategy report request");
    let mut client = TestClient::connect(&host);
    let first_request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(transport.clone()),
    });
    let first_receipt = client.send(&first_request);
    assert_eq!(first_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::StrategicPlanPrepared { plan: first_plan } =
        first_receipt.result().expect("first strategic plan result")
    else {
        panic!("expected first strategic plan result")
    };
    let first_plan = first_plan.as_ref().clone();
    let second_request = client.agent_request(RuntimeOperation::PrepareStrategicReport {
        request: Box::new(transport),
    });
    let second_receipt = client.send(&second_request);
    assert_eq!(second_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::StrategicPlanPrepared { plan: second_plan } = second_receipt
        .result()
        .expect("second strategic plan result")
    else {
        panic!("expected second strategic plan result")
    };
    assert_eq!(
        serde_json::to_vec(&first_plan).expect("first result bytes"),
        serde_json::to_vec(second_plan).expect("second result bytes")
    );

    let report_reference = first_plan.clone().into_parts().0;
    let artifact_sequence = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .find(|event| {
        event
            .artifacts
            .iter()
            .any(|artifact| artifact == &report_reference)
    })
    .expect("strategic report commit event")
    .sequence;
    let signal_events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signal_events.len(), 2);
    assert!(artifact_sequence < signal_events[0].sequence);
    let signals = signal_events
        .iter()
        .map(|event| {
            let ProjectionPayload::Full(payload) = &event.payload else {
                panic!("expected full planning signal payload")
            };
            let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal)) =
                payload.as_ref()
            else {
                panic!("expected planning signal payload")
            };
            signal
        })
        .collect::<Vec<_>>();
    assert_eq!(
        signals
            .iter()
            .map(|signal| signal.kind())
            .collect::<Vec<_>>(),
        vec![
            PolicyPlanningSignalKind::FeasibilityRed,
            PolicyPlanningSignalKind::TimelineReached,
        ]
    );
    assert_eq!(signals[0].fact_code(), "strategic.feasibility_red");
    assert_eq!(signals[1].fact_code(), "strategic.timeline_reached");
    assert_ne!(signals[0].signal_id(), signals[1].signal_id());
    assert!(signals.iter().all(|signal| {
        signal.signal_id().starts_with("signal:strategic:")
            && signal.signal_id().len() == 81
            && signal.instance_id() == POLICY_INSTANCE_ALIAS
            && signal.task_id().is_none()
            && signal.observed_at_unix_ms() == POLICY_NOW_UNIX_MS
            && signal.detection_budget().is_none()
    }));
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);

    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_report_is_local_deterministic_and_promotes_only_after_approval() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let aliases = [
        POLICY_INSTANCE_ALIAS.to_owned(),
        POLICY_INSTANCE_ALIAS_B.to_owned(),
    ];
    let mut input_facts = policy_facts();
    input_facts.instances.push(InstanceSnapshot {
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        server_id: "fixture-server-b".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    let host = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(
            input_facts.clone(),
            policy_resources(),
        )),
        Arc::new(FakeProvider::from_entries(
            aliases
                .into_iter()
                .map(|alias| (alias, instance_id(), Arc::clone(&fake_state))),
        )),
    )
    .expect("runtime host");
    let base_sources = strategy_policy_sources(1);
    let base = host
        .activate_policy_catalog(&base_sources)
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic pinned strategy evidence")
        .expect("strategy evidence");
    let (ledger_position, fact_snapshot_id) =
        strategic_frozen_identity(&host, &input_facts, &policy_resources());
    let mut facts = input_facts.clone();
    facts.ledger_position = ledger_position;
    facts.fact_snapshot_id = fact_snapshot_id;
    let report = strategy_report(&base, &evidence, &facts);

    let first = host
        .prepare_strategic_report(&report, std::slice::from_ref(&evidence))
        .expect("first strategy preparation");
    let second = host
        .prepare_strategic_report(&report, std::slice::from_ref(&evidence))
        .expect("replayed strategy preparation");
    assert_eq!(first, second);
    assert_eq!(first.report().kind(), ArtifactKind::StrategyReport);
    assert_eq!(first.projection().instances.len(), 2);
    assert!(first.projection().instances.iter().any(|projection| {
        projection.band == StrategicBand::InfeasibleBestEffort
            && projection.planning_disposition
                == actingcommand_policy::PlanningDisposition::ExecutionContinues
    }));
    assert_eq!(first.projection().additions.tasks.len(), 2);
    assert_eq!(first.projection().additions.activity_profiles.len(), 2);
    assert!(
        first
            .projection()
            .additions
            .tasks
            .iter()
            .all(|task| matches!(task.scope, ScopeSelector::Instance { .. }))
    );
    assert!(
        first
            .projection()
            .additions
            .activity_profiles
            .iter()
            .all(|profile| matches!(profile.scope, ScopeSelector::Instance { .. }))
    );
    let proposal = first.proposal().expect("mechanical catalog proposal");
    let (_, first_target_sources) =
        crate::proposal::prepare_proposal(&base, &base_sources, proposal)
            .expect("first proposal preparation")
            .into_ready()
            .expect("first proposal sources");
    assert_eq!(proposal.class(), ProposalClass::B);
    assert_eq!(proposal.report_refs(), [first.report().clone()]);
    assert_eq!(
        first
            .preview()
            .expect("strategy proposal preview")
            .proposal_id(),
        proposal.proposal_id()
    );
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    let stored =
        read_projected_verified(root.path(), first.report()).expect("local strategic report bytes");
    assert_eq!(
        serde_json::from_slice::<StrategicReport>(&stored).expect("stored strategic report"),
        report
    );

    let mut client = TestClient::connect(&host);
    let strategy_artifacts = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .flat_map(|event| event.artifacts)
    .filter(|artifact| artifact.kind() == ArtifactKind::StrategyReport)
    .count();
    assert_eq!(strategy_artifacts, 1);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::PolicyExecutionRecorded),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:strategy-plan",
        first
            .preview()
            .expect("strategy preview")
            .approval_target()
            .expect("strategy approval target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal.clone()),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_version(),
        2
    );

    let successor_base = host
        .active_policy_catalog()
        .expect("successor base catalog")
        .expect("successor base");
    let successor_evidence = host
        .store_test_report(b"synthetic successor strategy evidence")
        .expect("successor strategy evidence");
    let successor_resources = policy_resources();
    let (successor_position, successor_snapshot) =
        strategic_frozen_identity(&host, &input_facts, &successor_resources);
    let successor_report = strategy_report_with_assessments(
        &successor_base,
        &successor_evidence,
        successor_position,
        &successor_snapshot,
        vec![StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            game_id: "fixture-game-a".to_owned(),
            fact_snapshot_id: successor_snapshot.clone(),
            current_projection: Some(10),
            production_rate_per_hour: Some(100),
            target: 60,
            deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
            available: true,
            capability_ids: vec!["operation.observe".to_owned()],
        }],
    );
    let successor_first = host
        .prepare_strategic_report(&successor_report, std::slice::from_ref(&successor_evidence))
        .expect("successor strategy preparation");
    let successor_second = host
        .prepare_strategic_report(&successor_report, std::slice::from_ref(&successor_evidence))
        .expect("replayed successor strategy preparation");
    assert_eq!(successor_first, successor_second);
    let successor_proposal = successor_first
        .proposal()
        .expect("successor catalog proposal");
    assert_eq!(
        successor_proposal.report_refs(),
        [successor_first.report().clone()]
    );
    let ProposalKind::CatalogDiff { patches } = successor_proposal.proposal() else {
        panic!("expected successor catalog diff")
    };
    let removal_patches = patches
        .iter()
        .filter(|patch| patch.operation() == ProposalPatchOperation::Remove)
        .collect::<Vec<_>>();
    assert_eq!(removal_patches.len(), 2);

    let first_tasks: TasksDocument =
        serde_json::from_slice(&first_target_sources.tasks.bytes).expect("first target tasks");
    let first_activity: ActivityDocument =
        serde_json::from_slice(&first_target_sources.activity.bytes)
            .expect("first target activity");
    let prior_task_index = first_tasks
        .tasks
        .iter()
        .position(|task| {
            task.id.starts_with("strategy.task.")
                && matches!(
                    &task.scope,
                    ScopeSelector::Instance { instance_id }
                        if instance_id == POLICY_INSTANCE_ALIAS
                )
        })
        .expect("prior affected task index");
    let prior_profile_index = first_activity
        .profiles
        .iter()
        .position(|profile| {
            profile.id.starts_with("strategy.profile.")
                && matches!(
                    &profile.scope,
                    ScopeSelector::Instance { instance_id }
                        if instance_id == POLICY_INSTANCE_ALIAS
                )
        })
        .expect("prior affected profile index");
    let prior_task_path = format!("/tasks/{prior_task_index}");
    let prior_profile_path = format!("/profiles/{prior_profile_index}");
    assert_eq!(
        removal_patches
            .iter()
            .map(|patch| (patch.document(), patch.path()))
            .collect::<Vec<_>>(),
        vec![
            (ProposalDocument::Tasks, prior_task_path.as_str()),
            (ProposalDocument::Activity, prior_profile_path.as_str()),
        ]
    );
    let (_, successor_target_sources) = crate::proposal::prepare_proposal(
        &successor_base,
        &first_target_sources,
        successor_proposal,
    )
    .expect("successor proposal preparation")
    .into_ready()
    .expect("successor proposal sources");
    let successor_compiled =
        compile_catalog(&successor_target_sources).expect("successor target catalog");

    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(successor_proposal.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:strategy-successor",
        successor_first
            .preview()
            .expect("successor preview")
            .approval_target()
            .expect("successor approval target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(successor_proposal.clone()),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    let promoted = host
        .active_policy_catalog()
        .expect("promoted successor catalog")
        .expect("promoted successor");
    assert_eq!(promoted.catalog_version(), 3);
    assert_eq!(promoted.catalog_hash(), successor_compiled.catalog_hash());

    let successor_tasks: TasksDocument =
        serde_json::from_slice(&successor_target_sources.tasks.bytes).expect("successor tasks");
    let successor_activity: ActivityDocument =
        serde_json::from_slice(&successor_target_sources.activity.bytes)
            .expect("successor activity");
    let mut affected_task_ids = successor_tasks
        .tasks
        .iter()
        .filter(|task| {
            task.id.starts_with("strategy.task.")
                && matches!(
                    &task.scope,
                    ScopeSelector::Instance { instance_id }
                        if instance_id == POLICY_INSTANCE_ALIAS
                )
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    affected_task_ids.sort();
    let mut expected_task_ids = successor_first
        .projection()
        .additions
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    expected_task_ids.sort();
    assert_eq!(affected_task_ids, expected_task_ids);
    let mut affected_profile_ids = successor_activity
        .profiles
        .iter()
        .filter(|profile| {
            profile.id.starts_with("strategy.profile.")
                && matches!(
                    &profile.scope,
                    ScopeSelector::Instance { instance_id }
                        if instance_id == POLICY_INSTANCE_ALIAS
                )
        })
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    affected_profile_ids.sort();
    let mut expected_profile_ids = successor_first
        .projection()
        .additions
        .activity_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    expected_profile_ids.sort();
    assert_eq!(affected_profile_ids, expected_profile_ids);
    assert!(
        successor_tasks
            .tasks
            .iter()
            .any(|task| task.id == "fixture.observe")
    );
    assert!(
        successor_activity
            .profiles
            .iter()
            .any(|profile| profile.id == "fixture-activity-game")
    );
    for task in first.projection().additions.tasks.iter().filter(|task| {
        matches!(
            &task.scope,
            ScopeSelector::Instance { instance_id }
                if instance_id == POLICY_INSTANCE_ALIAS_B
        )
    }) {
        assert!(
            successor_tasks
                .tasks
                .iter()
                .any(|current| current.id == task.id)
        );
    }
    for profile in first
        .projection()
        .additions
        .activity_profiles
        .iter()
        .filter(|profile| {
            matches!(
                &profile.scope,
                ScopeSelector::Instance { instance_id }
                    if instance_id == POLICY_INSTANCE_ALIAS_B
            )
        })
    {
        assert!(
            successor_activity
                .profiles
                .iter()
                .any(|current| current.id == profile.id)
        );
    }
    let strategy_artifacts = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .flat_map(|event| event.artifacts)
    .filter(|artifact| artifact.kind() == ArtifactKind::StrategyReport)
    .count();
    assert_eq!(strategy_artifacts, 2);
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_report_rejects_unverified_evidence_without_artifact_or_catalog_change() {
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
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic pinned strategy evidence")
        .expect("strategy evidence");
    let evidence_sequence = verified_artifact_sequence(&host, &evidence);
    let base_facts = policy_facts();
    let (ledger_position, fact_snapshot_id) =
        strategic_frozen_identity(&host, &base_facts, &policy_resources());
    let mut facts = base_facts;
    facts.ledger_position = ledger_position;
    facts.fact_snapshot_id = fact_snapshot_id;
    let report = strategy_report(&base, &evidence, &facts);
    let stale_report = strategy_report_with_assessments(
        &base,
        &evidence,
        evidence_sequence - 1,
        &facts.fact_snapshot_id,
        report.assessments().to_vec(),
    );
    let error = host
        .prepare_strategic_report(&stale_report, std::slice::from_ref(&evidence))
        .expect_err("evidence newer than the report as-of position must fail");
    assert_eq!(error.code(), "strategic_evidence_unverified");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let forged = unverified_report(&evidence, &ids);

    let error = host
        .prepare_strategic_report(&report, &[forged])
        .expect_err("unverified strategy evidence must fail");
    assert_eq!(error.code(), "strategic_evidence_unverified");
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    let mut client = TestClient::connect(&host);
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
    drop(client);
    host.close().expect("close runtime");
}
