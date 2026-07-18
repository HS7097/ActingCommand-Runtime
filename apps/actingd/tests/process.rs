// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    AgentPayload, AgentSessionId, ApplicationLifecycleAction, EventActor, EventPayload, EventQuery,
    EventSource, EventType, FactScope, IdentifierIssuer, InstanceId, PolicyPayload,
    PolicyPlanningSignalEventData, PolicyPlanningSignalKind, ProjectInterfaceRequest,
    ProjectedArtifactReference, ProjectionPayload, ProjectionProfile, RUNTIME_INFO_FILE,
    RuntimeInfo, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CohortBudgets, Comparison, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactValue, ForwardProjectionConfig, HostResourceSnapshot,
    InstanceSnapshot, LoadProfile, MaintenanceDisposition, MaintenanceTrendPolicy, MetricRef,
    OutlierMetric, OutlierPolicy, PoolValueSnapshot, PredicateSpec, ScopeSelector, StrategicBand,
    StrategicEvidencePointer, StrategicGoal, StrategicInstanceAssessment, StrategicReport,
    StrategicTemplate,
};
use actingcommand_runtime_client::{
    PredictiveMaintenanceRequest, RuntimeClient, RuntimeClientConfig,
};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, CatalogGeneration, ExecutionBackendProvider, ResolvedExecutionInstance,
    RuntimeHost, RuntimeHostConfig,
};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

const INSTANCE_ALIAS: &str = "node.a";
const PROCESS_TEST_SALT: &str = "actingd-process-test-salt";
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _kill_result = self.0.kill();
            let _wait_result = self.0.wait();
        }
    }
}

#[test]
fn actingd_outlives_disposable_clients_and_accepts_reconnection() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    write_config(&config_path, root.path(), instance_id, false);
    let child = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actingd");
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());

    let first = connect(root.path());
    let owner_epoch = first.health().expect("first client health");
    drop(first);

    let second = connect(root.path());
    assert_eq!(second.health().expect("second client health"), owner_epoch);
    assert!(child.0.try_wait().expect("process state").is_none());
    drop(second);

    let agent = connect_agent(root.path());
    let wake_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_agent_wake_id()
        .expect("wake id")
        .transport();
    let error = agent
        .start_agent_session(wake_id)
        .expect_err("dispatcher must default to disabled");
    assert_eq!(
        error.projection().map(|projection| projection.code),
        Some(actingcommand_contract::RuntimeErrorCode::InvalidRequest)
    );
    drop(agent);

    child.0.kill().expect("kill actingd");
    assert!(!child.0.wait().expect("wait actingd").success());
}

#[test]
fn actingd_runs_configured_policy_through_decision_admission_and_lease() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    write_policy_config(&config_path, root.path(), instance_id);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());
    let client = wait_for_agent_client(&mut child.0, root.path());
    let started = Instant::now();
    let events = loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query policy startup events");
        if events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
        {
            break events;
        }
        if let Some(status) = child.0.try_wait().expect("process state") {
            panic!("actingd exited before policy lease with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "actingd policy startup timed out with events: {:?}",
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>()
        );
        thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    let admitted = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchAdmitted)
        .expect("policy admission event");
    let ProjectionPayload::Full(payload) = &admitted.payload else {
        panic!("expected forensic policy admission payload")
    };
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) = payload.as_ref() else {
        panic!("expected policy dispatch admission")
    };
    assert_eq!(payload.operation_id(), "operation.observe");
    assert_eq!(
        payload.package_digest(),
        format!("sha256:{}", "c".repeat(64))
    );
    assert!(!payload.procedure_binding_digest().is_empty());
    assert!(child.0.try_wait().expect("process state").is_none());

    drop(client);
    child.0.kill().expect("kill actingd");
    child.0.wait().expect("wait actingd");
}

#[test]
fn actingd_dispatcher_recovers_fake_backend_wake_and_replays_resume() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    // The fake provider creates durable wake state; the production daemon must recover it without
    // opening any device backend and must preserve the resume receipt across process replacement.
    seed_agent_wake(root.path(), instance_id);
    write_config(&config_path, root.path(), instance_id, true);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    let info = wait_for_runtime_info(&mut child.0, root.path());
    let client = connect_agent(root.path());
    let wakes = client
        .query_events(
            EventQuery {
                event_type: Some(EventType::AgentWakeRequested),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query wake events");
    let wake_id = wakes
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Agent(AgentPayload::WakeRequested(payload)) => {
                    Some(payload.wake().wake_id())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("agent wake id");
    let session = client
        .start_agent_session(wake_id)
        .expect("start agent session");
    let session_id = session.status().session_id();
    drop(client);

    let resume = agent_resume_request(session_id);
    let first = raw_exchange(&info, &resume);
    assert_eq!(first.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::AgentSessionObserved { context } = first.result().expect("resume result")
    else {
        panic!("expected resumed session")
    };
    assert_eq!(context.status().session_id(), session_id);
    assert_eq!(raw_exchange(&info, &resume), first);
    assert_eq!(resumed_event_count(root.path()), 1);

    child.0.kill().expect("kill first actingd");
    child.0.wait().expect("wait first actingd");
    drop(child);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    let restarted_info = wait_for_runtime_info(&mut child.0, root.path());
    assert_ne!(restarted_info.pid(), info.pid());
    assert_eq!(raw_exchange(&restarted_info, &resume), first);
    assert_eq!(resumed_event_count(root.path()), 1);

    child.0.kill().expect("kill restarted actingd");
    child.0.wait().expect("wait restarted actingd");
}

#[test]
fn actingd_exposes_typed_planning_capabilities_to_a_separate_client_process() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    let (base, evidence, evidence_sequence) = seed_planning_state(root.path(), instance_id);
    let report = strategic_report(&base, &evidence, evidence_sequence);
    write_config(&config_path, root.path(), instance_id, false);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());
    let client = connect_agent(root.path());

    let plan = client
        .prepare_strategic_report(&report, vec![evidence])
        .expect("prepare strategic report through daemon IPC");
    assert_eq!(plan.projection().catalog_version, base.catalog_version());
    assert_eq!(plan.projection().instances.len(), 1);

    let forward = client
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            ForwardProjectionConfig::for_hours(1, 32).expect("forward config"),
        )
        .expect("project policy through daemon IPC");
    assert_eq!(forward.catalog_version, base.catalog_version());

    let as_of_ledger_position = client
        .project_snapshot(ProjectInterfaceRequest::current())
        .expect("project ledger position")
        .ledger_position;
    let maintenance = client
        .assess_predictive_maintenance(
            PredictiveMaintenanceRequest::new(
                INSTANCE_ALIAS,
                "fixture.observe",
                FactScope::Instance {
                    instance_id: INSTANCE_ALIAS.to_owned(),
                },
                "resource.primary",
                as_of_ledger_position,
                POLICY_NOW_UNIX_MS,
                MaintenanceTrendPolicy::default(),
            )
            .expect("maintenance request"),
        )
        .expect("assess maintenance through daemon IPC");
    assert_eq!(
        maintenance.disposition,
        MaintenanceDisposition::EvidenceInsufficient
    );
    assert!(child.0.try_wait().expect("process state").is_none());

    drop(client);
    child.0.kill().expect("kill actingd");
    child.0.wait().expect("wait actingd");
}

#[test]
fn invalid_startup_returns_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", "missing-actingd-config.json"])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("FATAL actingd"));
}

#[test]
fn policy_startup_rejects_approval_ids_that_do_not_match_the_catalog() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_policy_config(&config_path, root.path(), instance_id());
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read policy config"))
            .expect("decode policy config");
    config["policy"]["catalog_approval_ids"] = json!(["approval:untrusted"]);
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("invalid policy config JSON"),
    )
    .expect("write invalid policy config");

    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("policy_catalog_approval_mismatch"));
    assert!(!root.path().join(RUNTIME_INFO_FILE).exists());
}

fn connect(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect runtime")
}

fn connect_agent(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Agent, EventSource::Adapter)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect agent runtime")
}

fn wait_for_agent_client(child: &mut Child, state_root: &Path) -> RuntimeClient {
    let started = Instant::now();
    loop {
        match RuntimeClient::connect(
            RuntimeClientConfig::new(state_root, EventActor::Agent, EventSource::Adapter)
                .with_io_timeout(Duration::from_millis(500)),
        ) {
            Ok(client) => return client,
            Err(error) => {
                if let Some(status) = child.try_wait().expect("process state") {
                    let mut stderr = String::new();
                    if let Some(pipe) = child.stderr.as_mut() {
                        pipe.read_to_string(&mut stderr)
                            .expect("read actingd stderr");
                    }
                    panic!("actingd exited before policy readiness with {status}: {stderr}");
                }
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "actingd policy connection timed out after {error}"
                );
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn write_config(path: &Path, state_root: &Path, instance_id: InstanceId, dispatcher_enabled: bool) {
    let mut value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": PROCESS_TEST_SALT,
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id,
            "application_id": "neutral.application",
            "adb_path": "adb",
            "touch_backend": "maatouch",
            "capture_backend": "adb",
            "push_touch_tool": false
        }]
    });
    if dispatcher_enabled {
        value["agent_dispatcher"] = json!({
            "max_attempts": 2,
            "max_session_ms": 60_000,
            "max_projection_events": 8
        });
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("config json"),
    )
    .expect("write config");
}

fn write_policy_config(path: &Path, state_root: &Path, instance_id: InstanceId) {
    let policy_root = state_root.join("policy");
    fs::create_dir(&policy_root).expect("create policy directory");
    let sources = actingd_policy_sources(1);
    for (name, source) in [
        ("tasks.json", sources.tasks),
        ("pools.json", sources.pools),
        ("activity.json", sources.activity),
        ("timeline.json", sources.timeline),
    ] {
        fs::write(policy_root.join(name), source.bytes).expect("write policy document");
    }
    let now_unix_ms = unix_ms_now();
    let value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": PROCESS_TEST_SALT,
        "governance_capability": "actingd-policy-bootstrap-capability",
        "policy": {
            "facts": configured_policy_facts(now_unix_ms),
            "resources": configured_policy_resources(now_unix_ms),
            "catalog": {
                "tasks": "policy/tasks.json",
                "pools": "policy/pools.json",
                "activity": "policy/activity.json",
                "timeline": "policy/timeline.json"
            },
            "catalog_approval_ids": ["approval:fixture-a"],
            "procedure_manifest": [{
                "procedure_ref": "procedure.observe",
                "package_digest": format!("sha256:{}", "c".repeat(64)),
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"]
            }]
        },
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id,
            "application_id": "neutral.application",
            "adb_path": "must-not-run-adb",
            "touch_backend": "maatouch",
            "capture_backend": "adb",
            "push_touch_tool": false
        }]
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("policy config json"),
    )
    .expect("write policy config");
}

fn wait_for_runtime_info(child: &mut Child, state_root: &Path) -> RuntimeInfo {
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(state_root.join(RUNTIME_INFO_FILE))
            && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&bytes)
            && info.pid() == child.id()
        {
            return info;
        }
        if let Some(status) = child.try_wait().expect("process state") {
            panic!("actingd exited before ready with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "actingd readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn start_actingd(config_path: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actingd")
}

fn seed_agent_wake(state_root: &Path, instance_id: InstanceId) {
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(state_root, PROCESS_TEST_SALT.as_bytes()).with_agent_dispatcher(
            AgentDispatcherConfig::new(2, 60_000, 8).expect("agent dispatcher config"),
        ),
        Arc::new(FakeProvider { instance_id }),
    )
    .expect("seed runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:actingd-process-dispatcher".to_owned(),
        instance_id: INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now(),
        detection_budget: None,
    })
    .expect("record planning signal");
    host.close().expect("close seed runtime host");
}

fn seed_planning_state(
    state_root: &Path,
    instance_id: InstanceId,
) -> (CatalogGeneration, ProjectedArtifactReference, u64) {
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(state_root, PROCESS_TEST_SALT.as_bytes()),
        Arc::new(PlanningSeedProvider { instance_id }),
    )
    .expect("seed planning runtime host");
    let base = host
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("activate planning catalog");
    let client = connect(state_root);
    let observation = client
        .observe_readonly(INSTANCE_ALIAS)
        .expect("seed verified planning evidence");
    let evidence = match observation.receipt().result() {
        Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => {
            observation.artifact().clone()
        }
        _ => panic!("expected readonly observation"),
    };
    let evidence_sequence = client
        .query_events(
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query verified planning evidence")
        .into_iter()
        .find(|event| event.artifacts.iter().any(|artifact| artifact == &evidence))
        .expect("verified planning evidence event")
        .sequence;
    drop(client);
    host.close().expect("close planning seed runtime");
    (base, evidence, evidence_sequence)
}

fn policy_sources(version: u64) -> CatalogSources {
    let mut sources = CatalogSources {
        tasks: CatalogDocumentSource::new(
            "memory://process/tasks.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json").to_vec(),
        ),
        pools: CatalogDocumentSource::new(
            "memory://process/pools.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json").to_vec(),
        ),
        activity: CatalogDocumentSource::new(
            "memory://process/activity.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                .to_vec(),
        ),
        timeline: CatalogDocumentSource::new(
            "memory://process/timeline.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
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
    sources
}

fn actingd_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("actingd task fixture");
    tasks["tasks"][0]["scope"] = json!({
        "kind": "instance",
        "instance_id": INSTANCE_ALIAS
    });
    tasks["tasks"][0]["trigger"]["predicates"][1]["scope"] = json!({
        "kind": "instance",
        "instance_id": INSTANCE_ALIAS
    });
    tasks["tasks"][0]["instance_overrides"] = json!([]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("actingd task bytes");

    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("actingd pool fixture");
    pools["pools"][0]["scope"] = json!({
        "kind": "instance",
        "instance_id": INSTANCE_ALIAS
    });
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("actingd pool bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("actingd activity fixture");
    activity["profiles"][0]["scope"] = json!({
        "kind": "instance",
        "instance_id": INSTANCE_ALIAS
    });
    activity["profiles"][0]["windows"][0]["start_minute_of_day"] = json!(0);
    activity["profiles"][0]["windows"][0]["end_minute_of_day"] = json!(0);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("actingd activity bytes");
    sources
}

fn strategy_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let game_scope = serde_json::json!({"kind": "game", "game_id": "fixture-game-a"});
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("strategy task fixture");
    tasks["tasks"][0]["scope"] = game_scope.clone();
    tasks["tasks"][0]["trigger"]["predicates"][1]["scope"] = game_scope.clone();
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("strategy task bytes");
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("strategy pool fixture");
    pools["pools"][0]["scope"] = game_scope;
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("strategy pool bytes");
    sources
}

fn strategic_report(
    base: &CatalogGeneration,
    evidence: &ProjectedArtifactReference,
    as_of_ledger_position: u64,
) -> StrategicReport {
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
            metric: MetricRef::Fact {
                fact_key: "resource.primary".to_owned(),
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
                match_bands: vec![StrategicBand::Actionable],
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
        vec![StrategicInstanceAssessment {
            goal_id: "goal.primary".to_owned(),
            instance_id: INSTANCE_ALIAS.to_owned(),
            game_id: "fixture-game-a".to_owned(),
            fact_snapshot_id: "snapshot:strategy-a".to_owned(),
            current_projection: Some(50),
            production_rate_per_hour: Some(100),
            target: 100,
            deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
            available: true,
            capability_ids: vec!["operation.observe".to_owned()],
        }],
        CohortBudgets {
            max_active: 1,
            max_prompt: 1,
        },
    )
    .expect("strategic report")
}

fn policy_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 1,
        fact_snapshot_id: "snapshot:process-a".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: INSTANCE_ALIAS.to_owned(),
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

fn configured_policy_facts(now_unix_ms: u64) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 0,
        fact_snapshot_id: "snapshot:actingd-config-a".to_owned(),
        facts: Vec::new(),
        outcomes: vec![actingcommand_policy::ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: INSTANCE_ALIAS.to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Boolean(false),
            observed_at_unix_ms: now_unix_ms,
        }],
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        }],
    }
}

fn configured_policy_resources(now_unix_ms: u64) -> EvaluationResources {
    EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: now_unix_ms,
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

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn agent_resume_request(session_id: AgentSessionId) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Agent,
        EventSource::Adapter,
        unix_ms_now(),
        RuntimeOperation::ResumeAgentSession { session_id },
    )
    .expect("resume request")
}

fn resumed_event_count(state_root: &Path) -> usize {
    connect_agent(state_root)
        .query_events(
            EventQuery {
                event_type: Some(EventType::AgentSessionResumed),
                ..EventQuery::default()
            },
            ProjectionProfile::Normal,
        )
        .expect("query resumed events")
        .len()
}

fn raw_exchange(info: &RuntimeInfo, request: &RuntimeRequest) -> RuntimeReceipt {
    let mut stream = TcpStream::connect(info.socket_addr().expect("runtime socket"))
        .expect("connect raw runtime client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");
    let body = serde_json::to_vec(request).expect("serialize runtime request");
    assert!(!body.is_empty() && body.len() <= 1024 * 1024);
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .expect("write request header");
    stream.write_all(&body).expect("write request body");
    stream.flush().expect("flush request");
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).expect("read receipt header");
    let length = u32::from_be_bytes(header) as usize;
    assert!((1..=1024 * 1024).contains(&length));
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).expect("read receipt body");
    let receipt = serde_json::from_slice::<RuntimeReceipt>(&body).expect("decode runtime receipt");
    receipt.validate().expect("validate runtime receipt");
    receipt
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis()
        .try_into()
        .expect("millisecond timestamp")
}

struct FakeProvider {
    instance_id: InstanceId,
}

struct PlanningSeedProvider {
    instance_id: InstanceId,
}

struct PlanningSeedCapture;

impl CaptureBackend for PlanningSeedCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        Frame::from_pixels(
            2,
            1,
            vec![255, 0, 0, 0, 255, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl ExecutionBackendProvider for PlanningSeedProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![INSTANCE_ALIAS.to_owned()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == INSTANCE_ALIAS)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "local-planning-seed"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        Err(DeviceError::fatal("planning seed opened input backend"))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Ok(Box::new(PlanningSeedCapture))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "planning seed controlled application backend",
        ))
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![INSTANCE_ALIAS.to_owned()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == INSTANCE_ALIAS)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        Err(DeviceError::fatal("fake input backend must not be opened"))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Err(DeviceError::fatal(
            "fake capture backend must not be opened",
        ))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "fake application backend must not be opened",
        ))
    }
}
