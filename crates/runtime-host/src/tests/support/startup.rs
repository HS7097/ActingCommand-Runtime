// SPDX-License-Identifier: AGPL-3.0-only

pub(super) fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

pub(super) fn config(root: &TempDir) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root.path(), b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest())
        .with_governance_capability(TEST_GOVERNANCE_CAPABILITY)
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

pub(super) fn host_with_state(root: &TempDir, alias: &str, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(alias, instance_id(), state)),
    )
    .expect("runtime host")
}

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
            expires_at_unix_ms: None,
            activity_window_id: None,
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

const TEST_GOVERNANCE_CAPABILITY: &str = "runtime-host-governance-test-capability";
const POLICY_INSTANCE_ALIAS: &str = "fixture-instance-a";
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;
