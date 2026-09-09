// SPDX-License-Identifier: AGPL-3.0-only

fn release_set(
    root: &Path,
    version: &str,
    marker: char,
) -> (RuntimeReleaseSet, ReleaseArtifactSources) {
    let source_root = root.join(format!("release-source-{version}-{marker}"));
    fs::create_dir(&source_root).expect("release source root");
    let runtime = source_root.join("runtime.bin");
    let ui = source_root.join("ui.bin");
    let resource = source_root.join("resource.bin");
    let runtime_bytes = format!("runtime:{version}:{marker}");
    let ui_bytes = format!("ui:{version}:{marker}");
    let resource_bytes = format!("resource:{version}:{marker}");
    fs::write(&runtime, runtime_bytes.as_bytes()).expect("runtime artifact");
    fs::write(&ui, ui_bytes.as_bytes()).expect("UI artifact");
    fs::write(&resource, resource_bytes.as_bytes()).expect("resource artifact");
    let manifest = RuntimeReleaseSet::new(
        version,
        format!("sha256:{:x}", Sha256::digest(runtime_bytes.as_bytes())),
        version,
        format!("sha256:{:x}", Sha256::digest(ui_bytes.as_bytes())),
        vec![
            ReleaseResourceVersion::new(
                "project-neutral",
                version,
                format!("sha256:{:x}", Sha256::digest(resource_bytes.as_bytes())),
            )
            .expect("resource version"),
        ],
    )
    .expect("release set");
    let sources = ReleaseArtifactSources::new(
        runtime,
        ui,
        BTreeMap::from([("project-neutral".to_owned(), resource)]),
    );
    (manifest, sources)
}

fn strategy_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let game_scope = serde_json::json!({"kind": "game", "game_id": "fixture-game-a"});
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("strategy task fixture");
    tasks["tasks"][0]["scope"] = game_scope.clone();
    tasks["tasks"][0]["trigger"]["predicates"][1]["scope"] = game_scope.clone();
    tasks["tasks"][0]["produces"][0]["amount"] = serde_json::json!(10);
    tasks["tasks"][0]["next_run_clamp_ms"] = serde_json::json!(1_000);
    tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(1_000);
    tasks["tasks"][0]["cooldown_ms"] = serde_json::json!(0);
    tasks["tasks"][0]["loop_budget"] = serde_json::json!({
        "daily_limit": 10,
        "window_iteration_limit": 5,
        "max_runtime_ms": 60_000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("strategy task bytes");
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("strategy pool fixture");
    pools["pools"][0]["scope"] = game_scope;
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("strategy pool bytes");
    sources
}

fn large_strategy_policy_sources(version: u64) -> CatalogSources {
    let mut sources = strategy_policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("large strategy task fixture");
    tasks["tasks"][0]["yield_points"] = serde_json::Value::Array(
        (0..128)
            .map(|index| {
                serde_json::Value::String(format!("checkpoint.{index:03}.{}", "x".repeat(108)))
            })
            .collect(),
    );
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("large strategy task bytes");
    sources
}

fn strategy_report(
    base: &CatalogGeneration,
    evidence: &ProjectedArtifactReference,
    facts: &EvaluationFacts,
) -> StrategicReport {
    strategy_report_with_assessments(
        base,
        evidence,
        facts.ledger_position,
        &facts.fact_snapshot_id,
        vec![
            StrategicInstanceAssessment {
                goal_id: "goal.primary".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                fact_snapshot_id: facts.fact_snapshot_id.clone(),
                current_projection: Some(10),
                production_rate_per_hour: Some(50),
                target: 50,
                deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                available: true,
                capability_ids: vec!["operation.observe".to_owned()],
            },
            StrategicInstanceAssessment {
                goal_id: "goal.primary".to_owned(),
                instance_id: "fixture-instance-b".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                fact_snapshot_id: facts.fact_snapshot_id.clone(),
                current_projection: Some(10),
                production_rate_per_hour: Some(50),
                target: 100,
                deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                available: true,
                capability_ids: vec!["operation.observe".to_owned()],
            },
        ],
    )
}

fn strategy_report_with_assessments(
    base: &CatalogGeneration,
    evidence: &ProjectedArtifactReference,
    as_of_ledger_position: u64,
    fact_snapshot_id: &str,
    mut assessments: Vec<StrategicInstanceAssessment>,
) -> StrategicReport {
    for assessment in &mut assessments {
        assessment.fact_snapshot_id = fact_snapshot_id.to_owned();
    }
    let max_active = u16::try_from(assessments.len()).expect("bounded assessment count");
    let artifact_id = serde_json::to_value(evidence.artifact_id)
        .expect("artifact id JSON")
        .as_str()
        .expect("artifact id string")
        .to_owned();
    StrategicReport::new(
        "fixture-game-a",
        base.catalog_hash(),
        base.catalog_version(),
        base.catalog_version() + 1,
        as_of_ledger_position,
        POLICY_NOW_UNIX_MS,
        format!("sha256:{}", "d".repeat(64)),
        format!("sha256:{}", "e".repeat(64)),
        vec![StrategicEvidencePointer {
            artifact_id,
            sha256: evidence.sha256.clone(),
        }],
        vec![StrategicGoal {
            goal_id: "goal.primary".to_owned(),
            goal_version: 1,
            metric: MetricRef::Pool {
                pool_id: "fixture-pool-a".to_owned(),
            },
            templates: vec![StrategicTemplate {
                template_id: "template.primary".to_owned(),
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
            }],
            outlier_policy: OutlierPolicy {
                metric: OutlierMetric::Shortfall,
                mad_multiplier_milli: 2_000,
                top_n: 1,
            },
        }],
        assessments,
        CohortBudgets {
            max_active,
            max_prompt: 1,
        },
    )
    .expect("strategic report")
}

fn proposal_version_patches(version: u64) -> Vec<CatalogDeclarationPatch> {
    [
        ProposalDocument::Tasks,
        ProposalDocument::Pools,
        ProposalDocument::Activity,
        ProposalDocument::Timeline,
    ]
    .into_iter()
    .map(|document| {
        CatalogDeclarationPatch::new(
            document,
            ProposalPatchOperation::Replace,
            "/catalog/catalog_version",
            Some(version.to_string()),
        )
        .expect("catalog version patch")
    })
    .collect()
}

fn unverified_report(
    reference: &ProjectedArtifactReference,
    ids: &IdentifierIssuer,
) -> ProjectedArtifactReference {
    let mut reference = reference.clone();
    reference.artifact_id = *ids
        .mint_artifact_id()
        .expect("unverified artifact id")
        .transport();
    let artifact_id = serde_json::to_value(reference.artifact_id)
        .expect("artifact id JSON")
        .as_str()
        .expect("artifact id string")
        .to_owned();
    reference.object_key = Some(format!(
        "artifacts/{}/{}.txt",
        &reference.sha256[7..9],
        artifact_id
    ));
    reference
}

fn verified_artifact_sequence(host: &RuntimeHost, reference: &ProjectedArtifactReference) -> u64 {
    let mut client = TestClient::connect(host);
    projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .find(|event| event.artifacts.iter().any(|artifact| artifact == reference))
    .expect("artifact verification event")
    .sequence
}

fn strategic_frozen_identity(
    host: &RuntimeHost,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> (u64, String) {
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            facts,
            resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            268,
            PolicyTrigger::FactsChanged,
        )
        .expect("authoritative strategy identity");
    let intent = cycle
        .evaluation
        .expect("strategy identity evaluation")
        .dispatch_intents
        .into_iter()
        .next()
        .expect("strategy identity intent");
    (intent.input_ledger_position, intent.fact_snapshot_id)
}

fn forward_projection_request(
    facts: &EvaluationFacts,
    config: ForwardProjectionConfig,
) -> RuntimeForwardProjectionRequest {
    RuntimeForwardProjectionRequest::new(
        RuntimePlanningDocument::encode(RuntimePlanningDocumentKind::EvaluationFacts, facts)
            .expect("evaluation facts document"),
        RuntimePlanningDocument::encode(
            RuntimePlanningDocumentKind::EvaluationResources,
            &policy_resources(),
        )
        .expect("evaluation resources document"),
        RuntimePlanningDocument::encode(
            RuntimePlanningDocumentKind::EvaluationTime,
            &EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
        )
        .expect("evaluation time document"),
        17,
        RuntimePlanningDocument::encode(
            RuntimePlanningDocumentKind::ForwardProjectionConfig,
            &config,
        )
        .expect("forward config document"),
    )
    .expect("forward projection request")
}
