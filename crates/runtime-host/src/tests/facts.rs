// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn runtime_fact_store_shares_server_facts_invalidates_and_recovers_from_ledger() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let provider = Arc::new(FakeProvider::one(
        POLICY_INSTANCE_ALIAS,
        registered_id,
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(config(&root), provider).expect("runtime host");
    let server_record = stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:server-theme",
        vec![EventType::PolicyPlanningSignalObserved],
    );
    let instance_record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "inventory.items",
        ContractFactValue::RecordList(Vec::new()),
        "snapshot:instance-inventory",
        Vec::new(),
    );
    let published = host
        .publish_fact(server_record.clone())
        .expect("publish server fact");
    assert_eq!(
        host.publish_fact(server_record.clone())
            .expect("idempotent fact publication"),
        published
    );
    host.publish_fact(instance_record)
        .expect("publish instance fact");

    let primary = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("primary snapshot");
    let peer = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: "fixture-instance-b".to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("peer snapshot");
    assert_eq!(primary.records.len(), 2);
    assert_eq!(peer.records.len(), 1);

    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:fact-invalidation".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.fixture.missed".to_owned(),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 1,
        detection_budget: None,
    })
    .expect("record invalidating event");
    let after = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("snapshot after invalidation");
    assert_eq!(after.records.len(), 1);
    assert_eq!(after.records[0].key, "inventory.items");
    let stale = host
        .publish_fact(server_record)
        .expect_err("invalidated source snapshot must not be resurrected");
    assert_eq!(stale.code(), "fact_source_snapshot_invalidated");
    assert!(!stale.is_fatal());

    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            }
        )
        .len(),
        2
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state,
        )),
    )
    .expect("reopen runtime host");
    let recovered = reopened
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("recovered snapshot");
    assert_eq!(recovered.records.len(), 1);
    assert_eq!(recovered.records[0].key, "inventory.items");
    reopened.close().expect("close reopened host");
}

#[test]
fn agent_adapter_publish_fact_uses_authoritative_fact_owner_once() {
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
    let record = stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:agent-adapter-publish",
        Vec::new(),
    );
    let mut client = TestClient::connect(&host);

    let first_request = client.agent_request(RuntimeOperation::PublishFact {
        record: record.clone(),
    });
    let first_receipt = client.send(&first_request);
    assert_eq!(first_receipt.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::FactPublished { event_id } =
        first_receipt.result().expect("fact publication result")
    else {
        panic!("expected fact publication result")
    };
    let published_event_id = *event_id;
    let first_events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        },
    );
    assert_eq!(first_events.len(), 1);
    assert_eq!(first_events[0].event_id, published_event_id);
    assert_eq!(
        first_events[0].links.request_id(),
        Some(&first_request.request_id())
    );
    assert_eq!(
        first_events[0].links.correlation_id(),
        Some(&first_request.correlation_id())
    );

    let duplicate = client.agent_request(RuntimeOperation::PublishFact {
        record: record.clone(),
    });
    let duplicate_receipt = client.send(&duplicate);
    assert!(matches!(
        duplicate_receipt.result(),
        Some(RuntimeResult::FactPublished { event_id }) if *event_id == published_event_id
    ));
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

    let invalid = client.agent_request(RuntimeOperation::PublishFact { record });
    let mut invalid = serde_json::to_value(invalid).expect("fact publication request JSON");
    invalid["operation"]["record"]["key"] = serde_json::json!("unsupported");
    let invalid: RuntimeRequest =
        serde_json::from_value(invalid).expect("invalid record transport shape");
    let invalid_receipt = client.send(&invalid);
    assert_eq!(invalid_receipt.state(), RuntimeReceiptState::Denied);
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
    assert!(host.fatal_error().expect("runtime health").is_none());
    let health = client.request(RuntimeOperation::Health);
    assert_eq!(client.send(&health).state(), RuntimeReceiptState::Completed);
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);

    // LIVE-FACT-POOL-v1 specification: atomic publication, retry, ordering and recovery.
    let a = stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".into(),
        },
        "resource.current",
        ContractFactValue::Integer(12),
        "snapshot:batch-a",
        Vec::new(),
    );
    let mut b = a.clone();
    b.key = "resource.capacity".into();
    b.content = FactContent::Inline {
        value: ContractFactValue::Integer(20),
    };
    let batch = actingcommand_contract::FactObservation {
        records: vec![a.clone(), b.clone()],
    };
    let adapter = actingcommand_runtime_client::RuntimeClient::connect(
        actingcommand_runtime_client::RuntimeClientConfig::new(
            root.path(),
            EventActor::Agent,
            EventSource::Adapter,
        ),
    )
    .unwrap();
    let batch_event = adapter
        .publish_facts(batch.clone())
        .expect("SDK batch publication");
    let mut reversed = batch.clone();
    reversed.records.reverse();
    let request = client.agent_request(RuntimeOperation::PublishFacts {
        observation: reversed,
    });
    assert!(
        matches!(client.send(&request).result(), Some(RuntimeResult::FactPublished { event_id }) if *event_id == batch_event)
    );
    let context = InstanceFactContext {
        instance_id: POLICY_INSTANCE_ALIAS.into(),
        server_id: "fixture-server-a".into(),
        game_id: "fixture-game-a".into(),
    };
    let snapshot = host.instance_fact_snapshot(context.clone()).unwrap();
    assert_eq!(
        snapshot
            .records
            .iter()
            .filter(|record| record.source_snapshot_id == "snapshot:batch-a")
            .count(),
        2
    );

    let mut refresh = batch.clone();
    for record in &mut refresh.records {
        record.source_snapshot_id = "snapshot:batch-b".into();
        record.observed_at_unix_ms += 1;
    }
    let request = client.agent_request(RuntimeOperation::PublishFact {
        record: refresh.records[0].clone(),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    let mut future = refresh.clone();
    for record in &mut future.records {
        record.observed_at_unix_ms = u64::MAX;
        record.expires_at_unix_ms = None;
        record.ttl_policy = None;
    }
    let request = client.agent_request(RuntimeOperation::PublishFacts {
        observation: future,
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    let request = client.agent_request(RuntimeOperation::PublishFacts {
        observation: refresh.clone(),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    let request = client.agent_request(RuntimeOperation::PublishFacts { observation: batch });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            }
        )
        .len(),
        3
    );
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    assert!(host.fatal_error().unwrap().is_none());
    drop(adapter);
    drop(client);
    host.close().expect("close host");
    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .unwrap();
    let recovered = reopened.instance_fact_snapshot(context).unwrap();
    let recovered = recovered
        .records
        .into_iter()
        .filter(|record| record.source_snapshot_id == "snapshot:batch-b")
        .collect::<Vec<_>>();
    assert_eq!(recovered.len(), 2);
    assert!(recovered.iter().all(
        |record| record.observed_at_unix_ms == POLICY_NOW_UNIX_MS + 1
            && record.expires_at_unix_ms == Some(POLICY_NOW_UNIX_MS + 60_000)
    ));
    reopened.close().unwrap();
}

#[test]
fn policy_evaluation_consumes_runtime_owned_fact_projection() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            state,
        )),
    )
    .unwrap();
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("tasks fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "server", "server_id": "fixture-server-a"},
        "fact_key": "env.ui_theme",
        "comparison": "eq",
        "value": {"type": "string", "value": "Neutral"},
        "max_age_ms": 60_000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate policy catalog");
    host.publish_fact(stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:policy-theme",
        Vec::new(),
    ))
    .expect("publish policy fact");

    let cycle = host
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
        .expect("evaluate fact-backed policy");
    let evaluation = cycle.evaluation.expect("policy evaluation");
    assert_eq!(evaluation.dispatch_intents.len(), 1);
    assert!(
        evaluation.dispatch_intents[0]
            .fact_snapshot_id
            .starts_with("snapshot:policy-fact:")
    );

    // LIVE-FACT-POOL-v1: live refresh and invalidation reach ordinary Host admission.
    let mut sources = policy_sources(2);
    let mut tasks: serde_json::Value = serde_json::from_slice(&sources.tasks.bytes).unwrap();
    tasks["tasks"][0]["trigger"] = serde_json::json!({"kind":"resource_projection","pool_id":"fixture-pool-a","comparison":"greater_than_or_equal","value":11});
    sources.tasks.bytes = serde_json::to_vec(&tasks).unwrap();
    let mut pools: serde_json::Value = serde_json::from_slice(&sources.pools.bytes).unwrap();
    pools["pools"][0]["value_source"] =
        serde_json::json!({"kind":"ledger_fact","minimum_confidence_milli":900});
    sources.pools.bytes = serde_json::to_vec(&pools).unwrap();
    host.activate_policy_catalog(&sources).unwrap();
    clock.advance(2_000);
    let mut resources = policy_resources();
    resources.pools.clear();
    let missing = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 2_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 2_000,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .unwrap();
    assert!(missing.evaluation.unwrap().dispatch_intents.is_empty());
    let mut current = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.into(),
        },
        "resource.primary",
        ContractFactValue::Integer(12),
        "snapshot:pool-current",
        vec![
            EventType::InputCommitted,
            EventType::InputFailed,
            EventType::PolicyPlanningSignalObserved,
        ],
    );
    current.observed_at_unix_ms += 2_000;
    host.publish_fact(current.clone()).unwrap();
    clock.advance(1_001);
    let available = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .unwrap()
        .evaluation
        .unwrap();
    let intent = &available.dispatch_intents[0];
    assert_eq!(
        intent.prerequisites.facts_fresh_until_unix_ms,
        current.expires_at_unix_ms
    );
    record_policy_approval(&host, intent);
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:pool-invalidated".into(),
        instance_id: POLICY_INSTANCE_ALIAS.into(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.fixture.missed".into(),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 3_001,
        detection_budget: None,
    })
    .unwrap();
    let reason = available
        .reason_chains
        .iter()
        .find(|reason| reason.id == intent.reason_chain_id)
        .unwrap();
    assert_eq!(
        host.admit_policy_dispatch(intent, reason, &policy_context(&host, intent))
            .unwrap_err()
            .code(),
        "policy_facts_stale"
    );
    clock.advance(1_001);
    assert!(
        host.evaluate_policy_cycle(PolicyTrigger::FactsChanged)
            .unwrap()
            .evaluation
            .unwrap()
            .dispatch_intents
            .is_empty()
    );
    clock.advance(1);
    current.observed_at_unix_ms = POLICY_NOW_UNIX_MS + 4_003;
    current.expires_at_unix_ms = Some(current.observed_at_unix_ms + 60_000);
    current.source_snapshot_id = "snapshot:pool-refreshed".into();
    host.publish_fact(current).unwrap();
    clock.advance(1_001);
    assert_eq!(
        host.evaluate_policy_cycle(PolicyTrigger::FactsChanged)
            .unwrap()
            .evaluation
            .unwrap()
            .dispatch_intents
            .len(),
        1
    );
    clock.advance(60_000);
    assert!(
        host.evaluate_policy_cycle(PolicyTrigger::Reconciliation)
            .unwrap()
            .evaluation
            .unwrap()
            .dispatch_intents
            .is_empty()
    );
    assert_eq!(
        host.evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 65_004,
                monotonic_ms: POLICY_NOW_UNIX_MS + 65_004
            },
            7,
            PolicyTrigger::FactsChanged
        )
        .unwrap_err()
        .code(),
        "policy_pool_authority_conflict"
    );
    host.close().expect("close host");
}

#[test]
fn fact_snapshot_catches_up_with_critical_ledger_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let primary_native = instance_id();
    let peer_native = instance_id();
    let peer_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                primary_native,
                state.clone(),
            ),
            (
                "fixture-instance-b".to_owned(),
                peer_native,
                peer_state.clone(),
            ),
        ])),
    )
    .unwrap();
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:critical-event",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");

    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("synchronized fact snapshot");
    assert!(snapshot.records.is_empty());

    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);

    // Defect regression: PR333 review 5133641794, D1 (LIVE-FACT-POOL-v1).
    // These are existing sealed backend inputs through the real lease/ledger path.
    let mut observation = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.current",
        ContractFactValue::Integer(12),
        "snapshot:before-first-input",
        vec![EventType::InputCommitted, EventType::InputFailed],
    );
    let mut fresh_event = None;
    for step in 0..3_u64 {
        clock.advance(1_000);
        let mut input_client = TestClient::connect(&host);
        let (_, token) = input_client.acquire(POLICY_INSTANCE_ALIAS);
        let input = input_client.request(RuntimeOperation::Input {
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        });
        assert_eq!(
            input_client.send(&input).state(),
            RuntimeReceiptState::Completed
        );
        let release = input_client.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            input_client.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        drop(input_client);
        if step != 1 {
            assert_eq!(
                host.publish_fact(observation.clone()).unwrap_err().code(),
                "fact_observation_precedes_input"
            );
        }
        clock.advance(1);
        observation.observed_at_unix_ms = POLICY_NOW_UNIX_MS + (step + 1) * 1_001;
        observation.expires_at_unix_ms = Some(observation.observed_at_unix_ms + 60_000);
        observation.source_snapshot_id = format!("snapshot:after-input-{step}");
        if step != 1 {
            fresh_event = Some(host.publish_fact(observation.clone()).unwrap());
        }
    }
    // Another native instance does not invalidate or advance this fact's boundary.
    clock.advance(1_000);
    let mut peer = TestClient::connect(&host);
    let (_, token) = peer.acquire("fixture-instance-b");
    let input = peer.request(RuntimeOperation::Input {
        token: token.clone(),
        action: InputAction::Tap { x: 10, y: 20 },
    });
    assert_eq!(peer.send(&input).state(), RuntimeReceiptState::Completed);
    let release = peer.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(peer.send(&release).state(), RuntimeReceiptState::Completed);
    drop(peer);
    assert_eq!(
        host.publish_fact(observation.clone()).unwrap(),
        fresh_event.unwrap()
    );
    host.close().expect("close host");
    let reopened = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::from_entries([
            (POLICY_INSTANCE_ALIAS.to_owned(), primary_native, state),
            ("fixture-instance-b".to_owned(), peer_native, peer_state),
        ])),
    )
    .unwrap();
    let restored = reopened
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .unwrap();
    assert!(restored.records.contains(&observation));
    observation.key = "resource.after_restart".to_owned();
    observation.source_snapshot_id = "snapshot:late-after-recovery".to_owned();
    observation.observed_at_unix_ms = POLICY_NOW_UNIX_MS + 2_002;
    observation.expires_at_unix_ms = Some(observation.observed_at_unix_ms + 60_000);
    assert_eq!(
        reopened.publish_fact(observation).unwrap_err().code(),
        "fact_observation_precedes_input"
    );
    reopened.close().unwrap();
}

#[test]
fn runtime_startup_materializes_a_missed_critical_fact_invalidation() {
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
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:restart-critical-event",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    host.close().expect("close without reading facts");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    let snapshot = reopened
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("recovered snapshot");
    assert!(snapshot.records.is_empty());
    let mut client = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);
    reopened.close().expect("close reopened host");
}
