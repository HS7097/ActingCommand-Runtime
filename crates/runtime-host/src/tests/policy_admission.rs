// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn policy_cadence_is_explicit_and_clock_jumps_force_full_recompute() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let facts = policy_facts();
    let resources = policy_resources();

    let startup = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("startup policy cycle");
    assert_eq!(startup.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        startup.directive.reason,
        PolicyRecomputeReason::StartupOrRecovery
    );
    assert!(startup.evaluation.is_some());
    let startup_measurement = startup.measurement.expect("startup measurement");
    assert_eq!(
        startup_measurement.requested_recompute,
        PolicyRecomputeKind::Full
    );
    assert_eq!(
        startup_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    assert!(startup_measurement.cost.work_units > 0);

    let cooldown = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 100,
            },
            7,
            PolicyTrigger::ResourcesChanged,
        )
        .expect("cooldown policy cycle");
    assert_eq!(cooldown.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(cooldown.directive.reason, PolicyRecomputeReason::Cooldown);
    assert!(cooldown.evaluation.is_none());
    assert!(cooldown.measurement.is_none());

    let incremental = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 1_100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("incremental policy cycle");
    assert_eq!(incremental.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(incremental.directive.reason, PolicyRecomputeReason::Event);
    let incremental_measurement = incremental.measurement.expect("incremental measurement");
    assert_eq!(
        incremental_measurement.requested_recompute,
        PolicyRecomputeKind::Incremental
    );
    assert_eq!(
        incremental_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    assert_eq!(incremental_measurement.cost, startup_measurement.cost);
    assert!(
        incremental_measurement.sampled_at_monotonic_ms
            >= startup_measurement.sampled_at_monotonic_ms
    );

    let clock_jump = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 7_000,
            },
            7,
            PolicyTrigger::ClockObserved {
                previous_unix_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
        )
        .expect("clock-jump policy cycle");
    assert_eq!(clock_jump.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        clock_jump.directive.reason,
        PolicyRecomputeReason::ClockJump
    );

    let reconciliation = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 67_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 67_000,
            },
            7,
            PolicyTrigger::Reconciliation,
        )
        .expect("reconciliation policy cycle");
    assert_eq!(reconciliation.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        reconciliation.directive.reason,
        PolicyRecomputeReason::Reconciliation
    );
    host.close().expect("close host");
}

#[test]
fn catalog_cas_conflict_preserves_nonfatal_identity_and_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let first = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let second = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");

    let error = host
        .activate_policy_catalog_with_expected_for_test(&policy_sources(3), first)
        .expect_err("stale compare-and-swap must fail");
    assert_eq!(error.code(), "catalog_active_generation_changed");
    assert_eq!(error.operation(), "switch_active_catalog");
    assert!(!error.is_fatal());
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("active generation"),
        second
    );

    let document = host
        .shared_ref("verify_catalog_state")
        .expect("live host")
        .state
        .read_json_document(actingcommand_runtime_state::CATALOG_ACTIVE_STATE_KEY)
        .expect("committed catalog state")
        .expect("active pointer");
    let pointer: serde_json::Value =
        serde_json::from_slice(document.payload()).expect("catalog pointer");
    assert_eq!(
        pointer["generation"]["catalog_hash"],
        second.catalog_hash(),
        "confirmed rollback preserves the durable pointer"
    );
    let mut client = TestClient::connect(&host);
    let failures = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogTransitionFailed),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) =
        &failures.last().expect("catalog transition failure").payload
    else {
        panic!("expected full catalog transition failure payload");
    };
    assert_eq!(
        payload.effect_disposition(),
        Some(EffectDisposition::NotPerformed)
    );

    host.activate_policy_catalog(&policy_sources(3))
        .expect("runtime remains usable after stale compare-and-swap");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let first_catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let forged_approval = policy_context(&host, &intent);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &forged_approval)
        .expect_err("caller-supplied approval IDs must not grant authority");
    assert_eq!(error.code(), "policy_approval_fact_missing");
    record_policy_approval(&host, &intent);

    let mut tampered_intent = intent.clone();
    tampered_intent.decision_id = "decision:tampered".to_owned();
    tampered_intent.reason_chain_id = "reason:tampered".to_owned();
    tampered_intent.approval_refs.clear();
    let mut tampered_reasons = reasons.clone();
    tampered_reasons.id = "reason:tampered".to_owned();
    tampered_reasons.decision_id = "decision:tampered".to_owned();
    let error = host
        .admit_policy_dispatch(
            &tampered_intent,
            &tampered_reasons,
            &policy_context(&host, &tampered_intent),
        )
        .expect_err("catalog approval requirements cannot be stripped");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    let (_, approved_intent, approved_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        8,
    );

    let admission = host
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&host, &approved_intent),
        )
        .expect("policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    let mut stale_context = policy_context(&host, &approved_intent);
    stale_context.now_unix_ms = POLICY_NOW_UNIX_MS + 60_000;
    let replay = host
        .admit_policy_dispatch(&approved_intent, &approved_reasons, &stale_context)
        .expect("exact replay is suppressed before mutable-state revalidation");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let after = projected_events(&mut client, EventQuery::default());
    assert_eq!(before.len(), after.len());
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
            .count(),
        2
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
            .count(),
        1
    );

    let second_catalog = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    assert_ne!(first_catalog.catalog_hash(), second_catalog.catalog_hash());
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut old_new_intent = intent.clone();
    old_new_intent.decision_id = "decision:fixture-b".to_owned();
    old_new_intent.reason_chain_id = "reason:fixture-b".to_owned();
    let mut old_new_reasons = reasons.clone();
    old_new_reasons.id = "reason:fixture-b".to_owned();
    old_new_reasons.decision_id = "decision:fixture-b".to_owned();
    let error = host
        .admit_policy_dispatch(
            &old_new_intent,
            &old_new_reasons,
            &policy_context(&host, &old_new_intent),
        )
        .expect_err("new admission cannot use the old catalog");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    host.complete_policy_dispatch(&approved_intent.decision_id)
        .expect("complete policy dispatch");
    assert!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .is_none()
    );
    let rolled_back = host
        .rollback_policy_catalog(first_catalog.catalog_hash())
        .expect("rollback policy catalog");
    assert_eq!(rolled_back, first_catalog);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogActivated)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogRolledBack)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchCompleted)
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );
    let replay = reopened
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&reopened, &approved_intent),
        )
        .expect("replay after restart");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close reopened host");

    // SCHEDULING-INSTANCE-ALIAS-v1; first red: Workflow #269 comment 5578055809.
    let alias = " Instance Ω ";
    let root = TempDir::new().expect("alias state");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let mut facts = policy_facts();
    facts.instances[0].instance_id = alias.to_owned();
    facts.outcomes[0].instance_id = alias.to_owned();
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources())),
        Arc::new(FakeProvider::one(alias, registered_id, Arc::clone(&state))),
    )
    .expect("alias host");
    let mut sources = policy_sources(1);
    let encoded = serde_json::to_string(alias).expect("alias JSON");
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        source.bytes = String::from_utf8(source.bytes.clone())
            .expect("source UTF-8")
            .replace("\"fixture-instance-a\"", &encoded)
            .into_bytes();
    }
    let mut tasks: serde_json::Value = serde_json::from_slice(&sources.tasks.bytes).unwrap();
    tasks["tasks"][0]["trigger"] = serde_json::json!({"kind":"fact","scope":{"kind":"instance","instance_id":alias},"fact_key":"env.alias_ready","comparison":"eq","value":{"type":"boolean","value":true},"max_age_ms":60000});
    sources.tasks.bytes = serde_json::to_vec(&tasks).unwrap();
    host.activate_policy_catalog(&sources)
        .expect("compile exact alias");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: alias.to_owned(),
        },
        "env.alias_ready",
        ContractFactValue::Boolean(true),
        "snapshot:alias-ready",
        Vec::new(),
    ))
    .expect("publish exact alias fact");
    let context = InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "fixture-server-a".to_owned(),
        game_id: "fixture-game-a".to_owned(),
    };
    assert_eq!(
        host.instance_fact_snapshot(context.clone())
            .expect("exact alias fact snapshot")
            .context
            .instance_id,
        alias
    );
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("alias evaluation");
    let evaluation = cycle.evaluation.expect("evaluation");
    let [intent] = evaluation.dispatch_intents.as_slice() else {
        panic!("one alias intent: {evaluation:?}")
    };
    assert_eq!(intent.instance_id, alias);
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|reason| reason.id == intent.reason_chain_id)
        .expect("reason chain");
    record_policy_approval(&host, intent);
    assert!(matches!(
        host.admit_policy_dispatch(intent, reasons, &policy_context(&host, intent))
            .expect("exact alias admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    assert!(matches!(
        host.admit_policy_dispatch(intent, reasons, &policy_context(&host, intent))
            .expect("exact alias replay"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let mut client = TestClient::connect(&host);
    for unknown in [" instance Ω ", "unknown.instance"] {
        let request = client.request(RuntimeOperation::ObserveReadonly {
            instance_alias: unknown.to_owned(),
        });
        let denied = client.send(&request);
        assert_eq!(denied.state(), RuntimeReceiptState::Denied);
        assert_eq!(
            denied.error_projection().expect("unknown alias").code,
            RuntimeErrorCode::InstanceUnknown
        );
    }
    let events = projected_events(&mut client, EventQuery::default());
    let admitted = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchAdmitted)
        .expect("native alias admission");
    assert_eq!(admitted.links.instance_id(), Some(&registered_id));
    let ProjectionPayload::Full(payload) = &admitted.payload else {
        panic!("full policy payload")
    };
    let EventPayload::Policy(actingcommand_contract::PolicyPayload::DispatchAdmitted(data)) =
        payload.as_ref()
    else {
        panic!("dispatch admission")
    };
    assert_eq!(data.instance_id(), alias);
    host.complete_policy_dispatch(&intent.decision_id)
        .expect("complete alias dispatch");
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close alias host");
    let reopened = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources())),
        Arc::new(FakeProvider::one(alias, registered_id, state)),
    )
    .expect("reopen exact alias");
    assert_eq!(
        reopened
            .instance_fact_snapshot(context)
            .expect("recovered alias fact")
            .context
            .instance_id,
        alias
    );
    assert!(matches!(
        reopened
            .admit_policy_dispatch(intent, reasons, &policy_context(&reopened, intent))
            .expect("recovered replay"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close alias replay host");
}

#[test]
fn policy_final_admission_records_the_actual_control_rejection() {
    // Workflow #269 B11 first red: issuecomment-5587376490.
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state.clone(),
        )),
    )
    .expect("control rejection host");
    let mut sources = policy_sources(1);
    let mut activity: serde_json::Value = serde_json::from_slice(&sources.activity.bytes).unwrap();
    let end_minute = (POLICY_NOW_UNIX_MS % 86_400_000) / 60_000 + 1;
    activity["profiles"][0]["windows"][0]["start_minute_of_day"] = serde_json::json!(0);
    activity["profiles"][0]["windows"][0]["end_minute_of_day"] = serde_json::json!(end_minute);
    sources.activity.bytes = serde_json::to_vec(&activity).unwrap();
    host.activate_policy_catalog(&sources)
        .expect("activate closing window");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    clock.advance(60_000);
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("capacity sample at final admission time");
    let failure = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("selection cannot bypass final locked window admission");
    assert_eq!(failure.code(), "policy_activity_window_closed");
    assert!(!failure.is_fatal());
    let events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchRejected),
            ..EventQuery::default()
        })
        .expect("actual rejected event");
    assert_eq!(events.len(), 1);
    let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = events[0].payload() else {
        panic!("rejection payload")
    };
    let rejection = payload.rejection().expect("original RequestFailure facts");
    assert_eq!(rejection, &failure.policy_rejection());
    assert_eq!(rejection.operation, "reserve_policy_budget");
    assert_eq!(
        rejection.next_eligible_unix_ms,
        Some((POLICY_NOW_UNIX_MS / 86_400_000 + 1) * 86_400_000)
    );
    assert!(rejection.budget.is_none());
    assert_eq!(payload.reasons().len(), reasons.reasons.len());
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        host.query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchAdmitted),
            ..EventQuery::default()
        })
        .unwrap()
        .is_empty()
    );
    let original = events[0].clone();
    host.close().expect("close rejected host");
    let restarted = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state,
        )),
    )
    .expect("replay rejection without changing ranking identity");
    let replayed = restarted
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchRejected),
            ..EventQuery::default()
        })
        .unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].payload(), original.payload());
    assert_eq!(replayed[0].sequence(), original.sequence());
    restarted.close().expect("close replayed host");
}

#[test]
fn policy_admission_rejects_stale_and_tampered_trusted_context() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("tasks fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "env.ephemeral",
        "comparison": "eq",
        "value": {"type": "string", "value": "Neutral"},
        "max_age_ms": 20
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog");
    let mut ephemeral = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ephemeral",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:ephemeral",
        Vec::new(),
    );
    ephemeral.expires_at_unix_ms = Some(POLICY_NOW_UNIX_MS + 20);
    ephemeral.ttl_policy = Some(FactTtlPolicy {
        minimum_ms: 1,
        maximum_ms: 100,
        source: FactTtlSource::DetectorContract,
    });
    host.publish_fact(ephemeral)
        .expect("publish ephemeral fact");

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut forged_decision = intent.clone();
    forged_decision.decision_id = "decision:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &forged_decision,
            &reasons,
            &policy_context(&host, &forged_decision),
        )
        .expect_err("forged decision identity must fail")
        .code(),
        "policy_decision_not_host_evaluated"
    );

    let mut wrong_profile = intent.clone();
    wrong_profile.prerequisites.activity_profile_id = "profile:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_profile,
            &reasons,
            &policy_context(&host, &wrong_profile),
        )
        .expect_err("caller-selected profile must fail")
        .code(),
        "policy_trusted_context_mismatch"
    );

    thread::sleep(Duration::from_millis(30));
    let mut forged_time = policy_context(&host, &intent);
    forged_time.now_unix_ms = 1;
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &forged_time)
            .expect_err("expired fact must fail admission")
            .code(),
        "policy_facts_stale"
    );
    host.close().expect("close host");
}

#[test]
fn runtime_owned_policy_inputs_supply_time_and_reject_unknown_resource_hosts() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 7_000));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock)
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("Runtime-owned policy evaluation");
    let intent = cycle
        .evaluation
        .expect("policy evaluation")
        .dispatch_intents
        .into_iter()
        .next()
        .expect("dispatch intent");
    assert_eq!(
        intent.prerequisites.evaluated_at_unix_ms,
        POLICY_NOW_UNIX_MS
    );
    host.close().expect("close host");

    let invalid_root = TempDir::new().expect("tempdir");
    let mut invalid_facts = policy_facts();
    invalid_facts.instances[0].host_id = "caller-forged-host".to_owned();
    let invalid = RuntimeHost::start(
        config(&invalid_root)
            .with_policy_inputs(PolicyInputSnapshot::new(invalid_facts, policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host with invalid policy inputs");
    invalid
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let error = invalid
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect_err("unknown resource host must fail");
    assert_eq!(error.code(), "policy_resource_metadata_untrusted");
    assert!(!error.is_fatal());
    invalid.close().expect("close invalid host");
}

#[test]
fn fact_replacement_invalidates_an_already_evaluated_dispatch() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    host.publish_fact(stored_fact(
        scope.clone(),
        "env.authority",
        ContractFactValue::Boolean(true),
        "snapshot:authority-a",
        Vec::new(),
    ))
    .expect("publish initial fact revision");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut replacement = stored_fact(
        scope,
        "env.authority",
        ContractFactValue::Boolean(true),
        "snapshot:authority-b",
        Vec::new(),
    );
    replacement.observed_at_unix_ms += 1;
    host.publish_fact(replacement)
        .expect("replace fact revision");

    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("superseded fact revision must invalidate dispatch");
    assert_eq!(error.code(), "policy_facts_stale");
    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn procedure_alias_rebinding_reports_package_digest_mismatch_before_lease() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    let original_package_digest = intent
        .package_digest
        .as_ref()
        .expect("bound package digest")
        .to_owned();
    record_policy_approval(&host, &intent);

    host.replace_procedure_manifest_for_test(procedure_manifest_with_primary(
        b"fixture procedure observe package v2",
        vec!["after_observation".to_owned()],
    ))
    .expect("replace trusted procedure manifest");
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("old alias binding must not reach lease admission");
    assert_eq!(error.code(), "procedure_package_digest_mismatch");

    let replacement = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 60_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 60_000,
            },
            7,
            PolicyTrigger::Reconciliation,
        )
        .expect("evaluate replacement binding")
        .evaluation
        .expect("replacement evaluation")
        .dispatch_intents
        .into_iter()
        .next()
        .expect("replacement intent");
    assert_ne!(replacement.decision_id, intent.decision_id);
    assert_ne!(
        replacement.package_digest.as_ref(),
        Some(&original_package_digest)
    );

    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn procedure_manifest_rejects_yield_point_mismatch_during_evaluation() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            b"fixture procedure observe package v1",
            vec!["different_boundary".to_owned()],
        )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let error = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect_err("manifest yield points must match the catalog intent");
    assert_eq!(error.code(), "procedure_yield_points_mismatch");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn policy_evaluation_fails_explicitly_without_a_procedure_manifest() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"missing-procedure-manifest-test")
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let error = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect_err("unconfigured manifest must not produce an unbound intent");
    assert_eq!(error.code(), "procedure_manifest_unconfigured");
    assert!(!error.is_fatal());
    host.close().expect("close host");
}

#[test]
fn concurrent_fact_replacement_and_admission_are_ledger_ordered() {
    let root = TempDir::new().expect("tempdir");
    let host = Arc::new(host_with_state(
        &root,
        POLICY_INSTANCE_ALIAS,
        Arc::new(FakeState::default()),
    ));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    host.publish_fact(stored_fact(
        scope.clone(),
        "env.concurrent_authority",
        ContractFactValue::Boolean(true),
        "snapshot:concurrent-a",
        Vec::new(),
    ))
    .expect("publish initial fact revision");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let barrier = Arc::new(Barrier::new(3));

    let publisher = {
        let host = Arc::clone(&host);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            let mut replacement = stored_fact(
                scope,
                "env.concurrent_authority",
                ContractFactValue::Boolean(true),
                "snapshot:concurrent-b",
                Vec::new(),
            );
            replacement.observed_at_unix_ms += 1;
            host.publish_fact(replacement)
        })
    };
    let admitting = {
        let host = Arc::clone(&host);
        let barrier = Arc::clone(&barrier);
        let context = policy_context(&host, &intent);
        let intent = intent.clone();
        let reasons = reasons.clone();
        thread::spawn(move || {
            barrier.wait();
            host.admit_policy_dispatch(&intent, &reasons, &context)
        })
    };
    barrier.wait();
    publisher
        .join()
        .expect("publisher thread")
        .expect("replacement fact");
    let admission = admitting.join().expect("admission thread");

    let mut client = TestClient::connect(&host);
    let replacement_sequence = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .map(|event| event.sequence)
    .max()
    .expect("replacement fact sequence");
    match admission {
        Ok(PolicyDispatchAdmission::Granted { .. }) => {
            let intent_sequence = projected_events(
                &mut client,
                EventQuery {
                    event_type: Some(EventType::PolicyDispatchIntent),
                    ..EventQuery::default()
                },
            )
            .into_iter()
            .map(|event| event.sequence)
            .max()
            .expect("policy intent sequence");
            assert!(intent_sequence < replacement_sequence);
        }
        Err(error) => {
            assert_eq!(error.code(), "policy_facts_stale");
            assert!(
                projected_events(
                    &mut client,
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    }
                )
                .is_empty()
            );
        }
        Ok(other) => panic!("unexpected admission: {other:?}"),
    }
    drop(client);
    let host = Arc::try_unwrap(host).unwrap_or_else(|_| panic!("exclusive runtime host"));
    host.close().expect("close host");
}

#[test]
fn game_and_server_scope_changes_cannot_reuse_a_trusted_decision_identity() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, trusted, trusted_reasons) =
        evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let mut wrong_instance = trusted.clone();
    wrong_instance.instance_id = "fixture-instance-b".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_instance,
            &trusted_reasons,
            &policy_context(&host, &wrong_instance),
        )
        .expect_err("instance scope cannot be widened by the caller")
        .code(),
        "policy_trusted_context_mismatch"
    );

    for (index, mut facts) in [policy_facts(), policy_facts()].into_iter().enumerate() {
        if index == 0 {
            facts.instances[0].server_id = "fixture-server-b".to_owned();
        } else {
            facts.instances[0].game_id = "fixture-game-b".to_owned();
        }
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                    monotonic_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                },
                8 + index as u64,
                PolicyTrigger::Reconciliation,
            )
            .expect("scope-changed policy evaluation");
        let changed = cycle
            .evaluation
            .expect("scope-changed evaluation")
            .dispatch_intents
            .into_iter()
            .next()
            .expect("scope-changed intent");
        assert_ne!(changed.fact_snapshot_id, trusted.fact_snapshot_id);
        assert_ne!(changed.decision_id, trusted.decision_id);

        let mut mixed = changed;
        mixed.decision_id = trusted.decision_id.clone();
        mixed.reason_chain_id = trusted.reason_chain_id.clone();
        assert_eq!(
            host.admit_policy_dispatch(&mixed, &trusted_reasons, &policy_context(&host, &mixed),)
                .expect_err("scope changes cannot reuse an old decision identity")
                .code(),
            "policy_trusted_context_mismatch"
        );
    }
    host.close().expect("close host");
}

#[test]
fn measured_contention_gates_deadline_dispatch_and_records_the_conflict() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    let mut sources = policy_sources(1);
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("activity fixture");
    activity["profiles"][0]["goals"][0]["deadline_unix_ms"] = serde_json::json!(POLICY_NOW_UNIX_MS);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog");
    host.observe_performance_control_for_test(PerformanceControlObservation {
        observed_at_unix_ms: POLICY_NOW_UNIX_MS - 2_000,
        host_responsiveness_basis_points: Some(7_000),
        third_party_pressure_basis_points: Some(0),
        foreground_fullscreen: false,
    })
    .expect("first contention sample");
    host.observe_performance_control_for_test(PerformanceControlObservation {
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        host_responsiveness_basis_points: Some(7_000),
        third_party_pressure_basis_points: Some(0),
        foreground_fullscreen: false,
    })
    .expect("second contention sample");
    assert_eq!(
        host.performance_control_directive(POLICY_INSTANCE_ALIAS)
            .expect("directive")
            .level,
        PerformanceControlLevel::DispatchPaused
    );

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.prerequisites.urgency_milli, 1_000);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("deadline must not bypass a measured contention gate");
    assert_eq!(error.code(), "performance_capacity_deadline_conflict");

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PerformanceBalanceChanged)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchRejected)
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
    );
    host.close().expect("close host");
}
