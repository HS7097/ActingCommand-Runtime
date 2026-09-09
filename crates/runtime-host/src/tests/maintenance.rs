// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn predictive_maintenance_reports_missing_evidence_without_a_signal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let as_of_ledger_position =
        project_snapshot(&host, ProjectInterfaceRequest::current()).ledger_position;
    let query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        as_of_ledger_position,
        POLICY_NOW_UNIX_MS,
        MaintenanceTrendPolicy::default(),
    )
    .expect("maintenance query");

    let assessment = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("maintenance assessment");
    assert_eq!(
        assessment.disposition,
        MaintenanceDisposition::EvidenceInsufficient
    );
    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::PolicyPlanningSignalObserved),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn predictive_maintenance_publishes_one_evidence_pinned_recheck_signal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let durations = [100_u64, 110, 200, 240];
    let confidences = [950_u16, 940, 800, 780];
    let fact_scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    let mut next_evaluation_at = POLICY_NOW_UNIX_MS;
    let mut last_observed_at = POLICY_NOW_UNIX_MS;

    for (index, (&duration_ms, &confidence_milli)) in
        durations.iter().zip(confidences.iter()).enumerate()
    {
        let clock = Arc::new(ManualRuntimeClock::new(next_evaluation_at, 0));
        let host = RuntimeHost::start(
            config(&root).with_runtime_clock(clock.clone()),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::clone(&state),
            )),
        )
        .expect("maintenance runtime host");
        if index == 0 {
            host.activate_policy_catalog(&policy_sources(1))
                .expect("activate policy catalog");
        }
        let mut facts = policy_facts();
        facts.fact_snapshot_id = format!("snapshot:maintenance-cycle-{index}");
        facts.outcomes[0].observed_at_unix_ms = next_evaluation_at;
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: next_evaluation_at,
                    monotonic_ms: next_evaluation_at,
                },
                100 + u64::try_from(index).expect("bounded maintenance index"),
                PolicyTrigger::Reconciliation,
            )
            .expect("maintenance policy evaluation");
        let evaluation = cycle.evaluation.expect("maintenance evaluation");
        let intent = evaluation
            .dispatch_intents
            .first()
            .expect("maintenance dispatch intent")
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("maintenance reason chain")
            .clone();
        if index == 0 {
            record_policy_approval(&host, &intent);
        }
        let admission = host
            .admit_policy_dispatch(
                &intent,
                &reasons,
                &PolicyAdmissionContext {
                    fact_ledger_position: intent.input_ledger_position,
                    fact_snapshot_id: intent.fact_snapshot_id.clone(),
                    approval_fact_ids: BTreeSet::new(),
                    fencing_owner_epoch: host.runtime_info().owner_epoch(),
                    now_unix_ms: next_evaluation_at,
                },
            )
            .expect("maintenance dispatch admission");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected maintenance dispatch admission")
        };
        let admission = context.admission();
        last_observed_at = next_evaluation_at + duration_ms;
        clock.advance(duration_ms);
        host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
            .expect("maintenance execution outcome");
        host.publish_fact(FactRecord {
            scope: fact_scope.clone(),
            key: "resource.primary".to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::Integer(10),
            },
            observed_at_unix_ms: last_observed_at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli,
            source_detector: "detector.maintenance".to_owned(),
            source_snapshot_id: format!("snapshot:maintenance-{index}"),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on: Vec::new(),
        })
        .expect("maintenance confidence fact");
        next_evaluation_at = admission.activity.next_eligible_unix_ms + 1;
        host.close().expect("close maintenance runtime host");
    }

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::clone(&state),
        )),
    )
    .expect("assessment runtime host");
    let assessment_at = last_observed_at + 1_000;
    let as_of_ledger_position =
        project_snapshot(&host, ProjectInterfaceRequest::current()).ledger_position;
    let query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        fact_scope,
        "resource.primary",
        as_of_ledger_position,
        assessment_at,
        MaintenanceTrendPolicy::default(),
    )
    .expect("maintenance query");
    let first = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("maintenance assessment");
    let second = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("replayed maintenance assessment");
    assert_eq!(first, second);
    assert_eq!(first.disposition, MaintenanceDisposition::RecheckSuggested);
    assert_eq!(first.duration_sample_count, 4);
    assert_eq!(first.confidence_sample_count, 4);
    let later_query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        as_of_ledger_position,
        assessment_at + 1_000,
        MaintenanceTrendPolicy::default(),
    )
    .expect("later maintenance query");
    let later = host
        .assess_and_publish_predictive_maintenance(&later_query)
        .expect("later maintenance assessment");
    assert_eq!(later.assessment_id, first.assessment_id);

    host.publish_fact(FactRecord {
        scope: FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        key: "resource.primary".to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(10),
        },
        observed_at_unix_ms: last_observed_at + 500,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 700,
        source_detector: "detector.maintenance".to_owned(),
        source_snapshot_id: "snapshot:maintenance-late".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    })
    .expect("publish backdated maintenance evidence");
    let pinned_after_late = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("pinned assessment after late event");
    assert_eq!(pinned_after_late, first);

    let advanced_ledger_position =
        project_snapshot(&host, ProjectInterfaceRequest::current()).ledger_position;
    let advanced_query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        advanced_ledger_position,
        assessment_at,
        MaintenanceTrendPolicy::default(),
    )
    .expect("advanced maintenance query");
    let advanced = host
        .assess_and_publish_predictive_maintenance(&advanced_query)
        .expect("advanced maintenance assessment");
    assert_eq!(advanced.confidence_sample_count, 5);
    assert_ne!(advanced.assessment_id, first.assessment_id);
    assert_eq!(advanced.as_of_ledger_position, advanced_ledger_position);

    let mut client = TestClient::connect(&host);
    let signals = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signals.len(), 2);
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close host");
}
