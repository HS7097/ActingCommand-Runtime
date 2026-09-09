// SPDX-License-Identifier: AGPL-3.0-only

fn procedure_manifest() -> ProcedureManifest {
    procedure_manifest_with_primary(
        b"fixture procedure observe package v1",
        vec!["after_observation".to_owned()],
    )
}

fn procedure_manifest_with_primary(
    primary_package: &[u8],
    primary_yield_points: Vec<String>,
) -> ProcedureManifest {
    ProcedureManifest::new(
        [
            "procedure.observe",
            "procedure.observe-b",
            "procedure.detect",
        ]
        .into_iter()
        .map(|procedure_ref| {
            let (package_digest, yield_points) = if procedure_ref == "procedure.observe" {
                (
                    format!("sha256:{:x}", Sha256::digest(primary_package)),
                    primary_yield_points.clone(),
                )
            } else {
                (
                    format!("sha256:{:x}", Sha256::digest(procedure_ref.as_bytes())),
                    vec!["after_observation".to_owned()],
                )
            };
            ProcedureBinding::new(
                procedure_ref,
                package_digest,
                "operation.observe",
                yield_points,
            )
            .expect("procedure binding")
        }),
    )
    .expect("procedure manifest")
}

const POLICY_INSTANCE_ALIAS: &str = "fixture-instance-a";
const POLICY_INSTANCE_ALIAS_B: &str = "fixture-instance-b";
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;
const MAPPED_RUN_MIN_ADVANCE_MS: u64 = 120_000;

fn policy_sources(version: u64) -> CatalogSources {
    let mut sources = CatalogSources {
        tasks: CatalogDocumentSource::new(
            "memory://fixture/tasks.json",
            include_bytes!("../../../../../contracts/scheduling/examples/catalog-a/tasks.json").to_vec(),
        ),
        pools: CatalogDocumentSource::new(
            "memory://fixture/pools.json",
            include_bytes!("../../../../../contracts/scheduling/examples/catalog-a/pools.json").to_vec(),
        ),
        activity: CatalogDocumentSource::new(
            "memory://fixture/activity.json",
            include_bytes!("../../../../../contracts/scheduling/examples/catalog-a/activity.json")
                .to_vec(),
        ),
        timeline: CatalogDocumentSource::new(
            "memory://fixture/timeline.json",
            include_bytes!("../../../../../contracts/scheduling/examples/catalog-a/timeline.json")
                .to_vec(),
        ),
    };
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: serde_json::Value =
            serde_json::from_slice(&source.bytes).expect("policy fixture JSON");
        document["catalog"]["catalog_version"] = serde_json::json!(version);
        source.bytes = serde_json::to_vec_pretty(&document).expect("policy fixture bytes");
    }
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("generic policy tasks");
    tasks["tasks"][0]["feedback_stop"] = serde_json::json!({
        "kind": "clock",
        "schedule": {
            "kind": "at",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "at_ms": 4102444800000_u64
        }
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("generic policy task bytes");
    sources
}

fn mapped_policy_sources(version: u64, outcome_key: &str) -> CatalogSources {
    mapped_policy_sources_with_keys(
        version,
        &[outcome_key, &complementary_outcome_key(outcome_key)],
    )
}

fn complementary_outcome_key(outcome_key: &str) -> String {
    format!("{outcome_key}-alternate")
}

fn mapped_policy_sources_with_keys(version: u64, outcome_keys: &[&str]) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("mapped policy tasks");
    // SCHEDULING-ELIGIBILITY-v1: repeated mapped runs use their actual activity
    // interval and task cooldown, with an explicitly recurrent source trigger.
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1, "anchor_ms": 0}
    });
    let mut followup = tasks["tasks"][0].clone();
    followup["id"] = serde_json::json!("fixture.followup");
    followup["procedure_ref"] = serde_json::json!("procedure.observe-b");
    followup["priority"] = serde_json::json!(200);
    followup["trigger"] = serde_json::json!({
        "kind": "any",
        "predicates": outcome_keys.iter().map(|outcome_key| serde_json::json!({
                "kind": "outcome",
                "task_id": "fixture.observe",
                "outcome_key": outcome_key,
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            })).chain(std::iter::once(serde_json::json!({
                    "kind": "clock",
                    "schedule": {
                        "kind": "at",
                        "clock_source": {
                            "kind": "server",
                            "timezone_id": "etc/utc",
                            "utc_offset_minutes": 0,
                            "dst_offset_minutes": 0,
                            "maintenance_drift_ms": 0
                        },
                        "at_ms": 4102444800000_u64
                    }
                }))).collect::<Vec<_>>()
    });
    followup["feedback_stop"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "fixture.followup.stop",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": 900000
    });
    followup["produces"] = serde_json::json!([]);
    followup["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("mapped tasks array")
        .push(followup);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("mapped policy task bytes");
    sources
}

fn mapped_two_key_any_policy_sources(
    version: u64,
    first_outcome_key: &str,
    second_outcome_key: &str,
) -> CatalogSources {
    let mut sources =
        mapped_policy_sources_with_keys(version, &[first_outcome_key, second_outcome_key]);
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("two-key activity fixture");
    // The Any recovery consumes a real ledger terminal at the runner's wall time.
    // Keep this fixture open every day while retaining its sampled interval and budgets.
    activity["profiles"][0]["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("two-key activity bytes");
    sources
}

fn budget_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("budget task fixture");
    tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(60000);
    tasks["tasks"][0]["cooldown_ms"] = serde_json::json!(0);
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1, "anchor_ms": 0}
    });
    tasks["tasks"][0]["loop_budget"] = serde_json::json!({
        "daily_limit": 4,
        "window_iteration_limit": 4,
        "max_runtime_ms": 300000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("budget task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("budget activity fixture");
    activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
    activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
    activity["profiles"][0]["session_max_ms"] = serde_json::json!(1000000);
    activity["profiles"][0]["minimum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["maximum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("budget activity bytes");
    sources
}

fn high_volume_forward_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("forward task fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": 1,
            "anchor_ms": 0
        }
    });
    tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(1);
    tasks["tasks"][0]["cooldown_ms"] = serde_json::json!(0);
    tasks["tasks"][0]["loop_budget"] = serde_json::json!({
        "daily_limit": 1_000_000,
        "window_iteration_limit": 1_000_000,
        "max_runtime_ms": 1_000_000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("forward task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("forward activity fixture");
    activity["profiles"][0]["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    activity["profiles"][0]["daily_budget"] = serde_json::json!(1_000_000);
    activity["profiles"][0]["max_window_iterations"] = serde_json::json!(1_000_000);
    activity["profiles"][0]["session_max_ms"] = serde_json::json!(1_000_000);
    activity["profiles"][0]["minimum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["maximum_interval_ms"] = serde_json::json!(1);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("forward activity bytes");
    sources
}

fn pending_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("pending task fixture");
    let mut second = tasks["tasks"][0].clone();
    second["id"] = serde_json::json!("fixture.observe-b");
    second["scope"] =
        serde_json::json!({"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS_B});
    second["procedure_ref"] = serde_json::json!("procedure.observe-b");
    second["produces"] = serde_json::json!([]);
    second["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("pending tasks array")
        .push(second);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("pending task bytes");
    sources
}

fn detection_policy_sources(version: u64) -> CatalogSources {
    detection_policy_sources_with_budget(version, 2, 20_000, 10_000)
}

fn detection_policy_sources_with_budget(
    version: u64,
    window_dispatch_limit: u32,
    window_runtime_ms: u64,
    expected_duration_ms: u64,
) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("detection task fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "ordinary.ready",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": 60000
    });
    let mut detection = tasks["tasks"][0].clone();
    detection["id"] = serde_json::json!("fixture.detect");
    detection["procedure_ref"] = serde_json::json!("procedure.detect");
    detection["priority"] = serde_json::json!(50);
    detection["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "detection.required",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": 60000
    });
    detection["produces"] = serde_json::json!([]);
    detection["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("detection tasks array")
        .push(detection);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("detection task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("detection activity fixture");
    activity["profiles"][0]["detection_budget"] = serde_json::json!({
        "window_dispatch_limit": window_dispatch_limit,
        "window_runtime_ms": window_runtime_ms,
        "expected_duration_ms": expected_duration_ms
    });
    sources.activity.bytes =
        serde_json::to_vec_pretty(&activity).expect("detection activity bytes");
    sources
}

fn evaluated_policy_dispatch(
    host: &RuntimeHost,
    trigger: PolicyTrigger,
) -> (PolicyCycle, DispatchIntent, DecisionReasonChain) {
    evaluated_policy_dispatch_at(host, trigger, POLICY_NOW_UNIX_MS, 7)
}

fn evaluated_policy_dispatch_at(
    host: &RuntimeHost,
    trigger: PolicyTrigger,
    unix_ms: u64,
    seed: u64,
) -> (PolicyCycle, DispatchIntent, DecisionReasonChain) {
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            trigger,
        )
        .expect("evaluate policy dispatch");
    let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
    let intent = evaluation
        .dispatch_intents
        .first()
        .unwrap_or_else(|| panic!("dispatch intent: {evaluation:#?}"))
        .clone();
    let reason_chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    (cycle, intent, reason_chain)
}

fn policy_context(host: &RuntimeHost, intent: &DispatchIntent) -> PolicyAdmissionContext {
    PolicyAdmissionContext {
        fact_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: BTreeSet::from(["approval:fixture-a".to_owned()]),
        fencing_owner_epoch: host.runtime_info().owner_epoch(),
        now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
    }
}

fn record_policy_approval(host: &RuntimeHost, intent: &DispatchIntent) -> TerminalEvent {
    record_policy_approval_disposition(host, intent, ApprovalDisposition::Approved)
}

fn record_policy_approval_disposition(
    host: &RuntimeHost,
    intent: &DispatchIntent,
    disposition: ApprovalDisposition,
) -> TerminalEvent {
    let decision = ApprovalDecisionRecord::new(
        "approval:fixture-a",
        disposition,
        ApprovalTarget::Catalog {
            catalog_hash: intent.catalog_hash.clone(),
            catalog_version: intent.catalog_version,
        },
        "user_confirmed",
    )
    .expect("approval decision");
    let mut client = TestClient::connect(host);
    client.authenticate_governance();
    let request = client.governance_request(RuntimeOperation::RecordApprovalDecision { decision });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ApprovalDecisionRecorded {
            approval_id,
            disposition: recorded,
        }) if approval_id == "approval:fixture-a" && *recorded == disposition
    ));
    receipt.terminal().expect("approval terminal")
}

fn record_target_approval(client: &mut TestClient, approval_id: &str, target: ApprovalTarget) {
    client.authenticate_governance();
    let decision = ApprovalDecisionRecord::new(
        approval_id,
        ApprovalDisposition::Approved,
        target,
        "proposal_reviewed",
    )
    .expect("proposal approval");
    let request = client.governance_request(RuntimeOperation::RecordApprovalDecision { decision });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
}

fn policy_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-a".to_owned(),
        facts: Vec::new(),
        outcomes: vec![ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Boolean(false),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        }],
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        }],
    }
}

fn mapped_policy_facts(outcome_key: &str, include_caller_outcome: bool) -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.fact_snapshot_id = format!(
        "snapshot:mapped:{outcome_key}:{}",
        if include_caller_outcome {
            "caller"
        } else {
            "authoritative"
        }
    );
    facts.outcomes.clear();
    if include_caller_outcome {
        facts.outcomes.push(ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            outcome_key: outcome_key.to_owned(),
            value: FactValue::Boolean(true),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        });
    }
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        fact_key: "fixture.followup.stop".to_owned(),
        value: FactValue::Boolean(false),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 900_000),
        confidence_milli: 1_000,
    });
    facts
}

fn pending_policy_facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.fact_snapshot_id = "snapshot:pending-a".to_owned();
    facts.outcomes.push(ObservedOutcome {
        task_id: "fixture.observe-b".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        outcome_key: "completed".to_owned(),
        value: FactValue::Boolean(false),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
    });
    facts.instances.push(InstanceSnapshot {
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        server_id: "fixture-server-b".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-b".to_owned(),
        available: true,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    facts
}

fn detection_policy_facts(ordinary_ready: bool, snapshot_id: &str) -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.fact_snapshot_id = snapshot_id.to_owned();
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        fact_key: "ordinary.ready".to_owned(),
        value: FactValue::Boolean(ordinary_ready),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        confidence_milli: 1_000,
    });
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        fact_key: "test.snapshot_revision".to_owned(),
        value: FactValue::String(snapshot_id.to_owned()),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        confidence_milli: 1_000,
    });
    facts
}

fn stored_fact(
    scope: FactScope,
    key: &str,
    value: ContractFactValue,
    source_snapshot_id: &str,
    invalidate_on: Vec<EventType>,
) -> FactRecord {
    FactRecord {
        scope,
        key: key.to_owned(),
        content: FactContent::Inline { value },
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 1_000,
            maximum_ms: 120_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 900,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: source_snapshot_id.to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on,
    }
}

fn policy_resources() -> EvaluationResources {
    EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        }],
        hosts: vec![HostResourceSnapshot {
            host_id: "fixture-host-a".to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
        }],
    }
}

fn pending_policy_resources() -> EvaluationResources {
    let mut resources = policy_resources();
    resources.hosts.push(HostResourceSnapshot {
        host_id: "fixture-host-b".to_owned(),
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
    });
    resources
}
