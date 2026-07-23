// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    AgentPayload, AgentSessionId, ApplicationLifecycleAction, EffectDisposition, EventActor,
    EventPayload, EventQuery, EventSource, EventType, FactScope, IdentifierIssuer, InputPayload,
    InstanceId, MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, PolicyExecutionOutcome,
    PolicyFailureClass, PolicyFailureDisposition, PolicyPayload, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind,
    ProjectInterfaceRequest, ProjectedArtifactReference, ProjectionPayload, ProjectionProfile,
    RUNTIME_INFO_FILE, RuntimeErrorCode, RuntimeEventQueryPageRequest, RuntimeInfo,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TaskPayload, TaskSemanticFact,
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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

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
fn actingd_preserves_legacy_physical_policy_intent_admission_and_lease() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_legacy_physical_policy_config(&config_path, root.path(), instance_id());

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());
    let client = wait_for_agent_client(&mut child.0, root.path());
    let started = Instant::now();
    let events = loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query legacy policy startup events");
        if events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
        {
            break events;
        }
        if let Some(status) = child.0.try_wait().expect("process state") {
            panic!("actingd exited before legacy policy lease with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "legacy policy startup timed out"
        );
        thread::sleep(Duration::from_millis(20));
    };
    for event_type in [
        EventType::PolicyDispatchIntent,
        EventType::PolicyDispatchAdmitted,
        EventType::LeaseGranted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "legacy {event_type:?}"
        );
    }
    for event_type in [
        EventType::LabRequest,
        EventType::TaskRequested,
        EventType::TaskCompleted,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            0,
            "legacy policy must not enter fixture execution: {event_type:?}"
        );
    }
    let admitted = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchAdmitted)
        .expect("legacy policy admission event");
    let ProjectionPayload::Full(payload) = &admitted.payload else {
        panic!("legacy forensic policy admission payload")
    };
    let EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) = payload.as_ref() else {
        panic!("legacy policy dispatch admission")
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
fn actingd_rejects_explicit_fixture_simulation_on_a_physical_registry() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    write_policy_execution_config(
        &config_path,
        root.path(),
        instance_id,
        &[vec![0, 0, 255, 0, 255, 0]],
        0,
    );
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read fixture config"))
            .expect("decode fixture config");
    config["instances"] = json!([{
        "alias": INSTANCE_ALIAS,
        "instance_id": instance_id,
        "application_id": "neutral.application",
        "adb_path": "must-not-run-adb",
        "touch_backend": "maatouch",
        "capture_backend": "adb",
        "push_touch_tool": false
    }]);
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("physical policy config JSON"),
    )
    .expect("write physical policy config");

    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("fixture_simulation_requires_fixture_backend")
    );
    assert!(!root.path().join(RUNTIME_INFO_FILE).exists());
}

#[test]
fn actingd_requires_package_path_only_for_explicit_fixture_simulation() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_policy_execution_config(
        &config_path,
        root.path(),
        instance_id(),
        &[vec![0, 0, 255, 0, 255, 0]],
        0,
    );
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read fixture config"))
            .expect("decode fixture config");
    config["policy"]["procedure_manifest"][0]["scheduled_execution"]
        .as_object_mut()
        .expect("scheduled execution object")
        .remove("package_path");
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("missing package config JSON"),
    )
    .expect("write missing package config");

    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("procedure_package_path_missing"));
    assert!(!root.path().join(RUNTIME_INFO_FILE).exists());
}

#[test]
fn actingd_does_not_execute_fixture_without_an_explicit_scheduled_binding() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_policy_execution_config(
        &config_path,
        root.path(),
        instance_id(),
        &[vec![0, 0, 255, 0, 255, 0]],
        0,
    );
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read fixture config"))
            .expect("decode fixture config");
    config["policy"]["procedure_manifest"][0]
        .as_object_mut()
        .expect("procedure binding object")
        .remove("scheduled_execution");
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("admission-only fixture config JSON"),
    )
    .expect("write admission-only fixture config");

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());
    let client = wait_for_agent_client(&mut child.0, root.path());
    let started = Instant::now();
    let events = loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query admission-only fixture events");
        if events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
        {
            break events;
        }
        if let Some(status) = child.0.try_wait().expect("process state") {
            panic!("actingd exited before admission-only fixture lease with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "admission-only fixture policy timed out"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
            .count(),
        1
    );
    for event_type in [
        EventType::LabRequest,
        EventType::TaskRequested,
        EventType::TaskCompleted,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            0,
            "untyped fixture binding must not execute: {event_type:?}"
        );
    }
    assert!(child.0.try_wait().expect("process state").is_none());

    drop(client);
    child.0.kill().expect("kill actingd");
    child.0.wait().expect("wait actingd");
}

#[test]
fn actingd_closes_one_policy_run_through_fixture_receipt_ledger_and_report_inputs() {
    let expected_package_sha256 = format!("{:x}", Sha256::digest(neutral_contained_task_package()));
    let expected_package_digest = format!("sha256:{expected_package_sha256}");
    let (_actinglab_target, actinglab_binary) = build_candidate_actinglab();
    for (case, frames, max_inputs, expected_effects) in [
        (
            "effecting",
            vec![vec![255, 0, 0, 0, 255, 0], vec![0, 0, 255, 0, 255, 0]],
            1,
            1,
        ),
        ("no-op", vec![vec![0, 0, 255, 0, 255, 0]], 0, 0),
    ] {
        let root = TempDir::new().expect("tempdir");
        let config_path = root.path().join("actingd.json");
        write_policy_execution_config(
            &config_path,
            root.path(),
            instance_id(),
            &frames,
            max_inputs,
        );

        let child = start_actingd(&config_path);
        let mut child = ChildGuard(child);
        wait_for_runtime_info(&mut child.0, root.path());
        let client = wait_for_agent_client(&mut child.0, root.path());
        let started = Instant::now();
        let events = loop {
            let queried = client.query_events(EventQuery::default(), ProjectionProfile::Forensic);
            let events = match queried {
                Ok(events) => events,
                Err(error) => {
                    if let Some(status) = child.0.try_wait().expect("process state") {
                        let mut stderr = String::new();
                        if let Some(pipe) = child.0.stderr.as_mut() {
                            pipe.read_to_string(&mut stderr)
                                .expect("read actingd stderr");
                        }
                        panic!(
                            "{case}: actingd exited while querying with {status}: {error}: {stderr}"
                        );
                    }
                    assert!(
                        started.elapsed() < Duration::from_secs(5),
                        "{case}: query run events failed: {error}"
                    );
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            if events
                .iter()
                .any(|event| event.event_type == EventType::PolicyDispatchCompleted)
            {
                break events;
            }
            if let Some(status) = child.0.try_wait().expect("process state") {
                let mut stderr = String::new();
                if let Some(pipe) = child.0.stderr.as_mut() {
                    pipe.read_to_string(&mut stderr)
                        .expect("read actingd stderr");
                }
                panic!("{case}: actingd exited before completed run with {status}: {stderr}");
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{case}: completed policy run timed out"
            );
            thread::sleep(Duration::from_millis(20));
        };

        let intent = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchIntent)
            .unwrap_or_else(|| panic!("{case}: policy intent"));
        let run_id = intent.links.run_id().copied().expect("run id");
        let task_id = intent.links.task_id().copied().expect("task id");
        let correlation_id = intent
            .links
            .correlation_id()
            .copied()
            .expect("correlation id");
        let run_events = events
            .iter()
            .filter(|event| event.links.run_id() == Some(&run_id))
            .collect::<Vec<_>>();
        let intent_payload = match &intent.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => payload,
                _ => panic!("{case}: policy intent payload"),
            },
            _ => panic!("{case}: forensic policy intent"),
        };
        assert!(
            run_events
                .iter()
                .all(|event| event.origin.source() != EventSource::Device),
            "{case}: fixture simulation must not emit device-origin events"
        );
        assert!(
            run_events.iter().all(|event| {
                event.links.task_id() == Some(&task_id)
                    && event.links.correlation_id() == Some(&correlation_id)
            }),
            "{case}: every run-linked event must share task and correlation identity"
        );
        for event_type in [
            EventType::PolicyDispatchIntent,
            EventType::PolicyDispatchAdmitted,
            EventType::LeaseGranted,
            EventType::LabRequest,
            EventType::TaskRequested,
            EventType::TaskCompleted,
            EventType::LeaseReleased,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                run_events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                1,
                "{case}: {event_type:?}"
            );
        }
        let package_admitted = run_events
            .iter()
            .find(|event| event.event_type == EventType::TaskRequested)
            .expect("package admitted fact");
        assert!(matches!(
            &package_admitted.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Task(TaskPayload::Semantic(payload))
                        if matches!(
                            payload.fact(),
                            TaskSemanticFact::PackageAdmitted { package_sha256, .. }
                                if package_sha256 == &expected_package_sha256
                        )
                )
        ));
        assert_eq!(
            run_events
                .iter()
                .filter(|event| event.event_type == EventType::TaskEffectCompleted)
                .count(),
            expected_effects,
            "{case}: effect count"
        );
        let simulation_events = run_events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::CaptureRequested
                        | EventType::CaptureCompleted
                        | EventType::InputIntent
                        | EventType::InputCommitted
                )
            })
            .collect::<Vec<_>>();
        assert!(
            !simulation_events.is_empty()
                && simulation_events.iter().all(|event| {
                    event.origin.source() == EventSource::Lab
                        && event.origin.module() == OriginModule::Actinglab
                }),
            "{case}: captures and simulated inputs must carry Lab/Actinglab provenance"
        );
        let committed_inputs = simulation_events
            .iter()
            .filter(|event| event.event_type == EventType::InputCommitted)
            .collect::<Vec<_>>();
        assert_eq!(
            committed_inputs.len(),
            expected_effects,
            "{case}: input count"
        );
        assert!(
            committed_inputs.iter().all(|event| {
                matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if matches!(
                            payload.as_ref(),
                            EventPayload::Input(InputPayload::Committed(outcome))
                                if outcome.effect_disposition() == EffectDisposition::NotPerformed
                        )
                )
            }),
            "{case}: fixture input must be recorded as not performed"
        );
        let lease_id = run_events
            .iter()
            .find(|event| event.event_type == EventType::LeaseGranted)
            .and_then(|event| event.links.lease_id())
            .copied()
            .expect("lease id");
        let admitted_payload = run_events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchAdmitted)
            .and_then(|event| match &event.payload {
                ProjectionPayload::Full(payload) => match payload.as_ref() {
                    EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)) => Some(payload),
                    _ => None,
                },
                _ => None,
            })
            .expect("policy admission payload");
        assert!(
            run_events
                .iter()
                .filter(|event| {
                    matches!(
                        event.event_type,
                        EventType::TaskCompleted
                            | EventType::LeaseReleased
                            | EventType::PolicyExecutionRecorded
                            | EventType::PolicyDispatchCompleted
                    )
                })
                .all(|event| event.links.lease_id() == Some(&lease_id))
        );
        let lab_request = run_events
            .iter()
            .find(|event| event.event_type == EventType::LabRequest)
            .expect("lab request");
        let task_completed = run_events
            .iter()
            .find(|event| event.event_type == EventType::TaskCompleted)
            .expect("task completed");
        let lab_request_id = lab_request
            .links
            .request_id()
            .copied()
            .expect("lab request id");
        let terminal_request_id = task_completed
            .links
            .request_id()
            .copied()
            .expect("task completed request id");
        assert_eq!(
            lab_request_id, terminal_request_id,
            "{case}: receipt terminal must use the contained request identity"
        );
        let execution = run_events
            .iter()
            .find(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .expect("policy execution");
        let ProjectionPayload::Full(payload) = &execution.payload else {
            panic!("{case}: forensic execution payload")
        };
        let EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) = payload.as_ref()
        else {
            panic!("{case}: policy execution payload")
        };
        assert!(matches!(
            payload.outcome(),
            PolicyExecutionOutcome::Succeeded { .. }
        ));
        let summary = client
            .summarize_run(run_id)
            .unwrap_or_else(|error| panic!("{case}: summarize completed run: {error}"));
        assert_eq!(
            summary.get("status").and_then(serde_json::Value::as_str),
            Some("simulated_completed")
        );
        assert_eq!(
            summary
                .get("package_digest")
                .and_then(serde_json::Value::as_str),
            Some(expected_package_digest.as_str())
        );
        assert_eq!(
            summary.get("effect").and_then(serde_json::Value::as_str),
            Some(if expected_effects == 1 {
                "would_effect"
            } else {
                "no_op"
            })
        );
        assert_eq!(
            summary
                .get("actual_effect_count")
                .and_then(serde_json::Value::as_u64),
            Some(0)
        );
        assert_eq!(
            summary
                .get("simulated_effect_count")
                .and_then(serde_json::Value::as_u64),
            Some(expected_effects as u64)
        );
        let provenance = summary
            .get("execution_provenance")
            .expect("structured execution provenance");
        assert_eq!(
            provenance.get("kind").and_then(serde_json::Value::as_str),
            Some("fixture_simulation")
        );
        for field in ["device_access", "account_access", "production_input"] {
            assert_eq!(
                provenance.get(field).and_then(serde_json::Value::as_bool),
                Some(false),
                "{case}: {field}"
            );
        }
        assert_eq!(
            summary
                .get("request")
                .and_then(|request| request.get("lab_request_id")),
            summary
                .get("request")
                .and_then(|request| request.get("receipt_request_id")),
            "{case}: report must preserve the receipt request identity"
        );
        assert_eq!(
            summary.get("run_id"),
            Some(&json!(run_id)),
            "{case}: report run must match the source event"
        );
        assert_eq!(
            summary.get("task_id"),
            Some(&json!(task_id)),
            "{case}: report task must match the source event"
        );
        assert_eq!(
            summary.get("correlation_id"),
            Some(&json!(correlation_id)),
            "{case}: report correlation must match the source event"
        );
        assert_eq!(
            summary.pointer("/request/lab_request_id"),
            Some(&json!(lab_request_id)),
            "{case}: report request must match the source LabRequest event"
        );
        assert_eq!(
            summary.pointer("/request/receipt_request_id"),
            Some(&json!(terminal_request_id)),
            "{case}: report receipt must match the source TaskCompleted event"
        );
        assert_eq!(
            summary.get("decision_id"),
            Some(&json!(intent_payload.decision_id())),
            "{case}: report decision must come from the run's intent event"
        );
        assert_eq!(
            summary.get("admission"),
            Some(
                &serde_json::to_value(
                    admitted_payload
                        .admission()
                        .expect("admitted event admission record"),
                )
                .expect("admission JSON")
            ),
            "{case}: report admission must come from the run's admitted event"
        );
        assert_eq!(
            summary.pointer("/lease/lease_id"),
            Some(&serde_json::to_value(lease_id).expect("lease id JSON")),
            "{case}: report lease must match the run's granted lease"
        );
        assert_eq!(
            summary.pointer("/reason_chain/id"),
            Some(&json!(intent_payload.reason_chain_id())),
            "{case}: report reason-chain identity must come from the run's intent"
        );
        assert_eq!(
            summary.pointer("/reason_chain/reasons"),
            Some(
                &serde_json::to_value(intent_payload.reasons())
                    .expect("policy intent reasons JSON")
            ),
            "{case}: report reasons must come from the run's intent"
        );
        assert_eq!(
            summary.pointer("/outcome/policy"),
            Some(&serde_json::to_value(payload.outcome()).expect("policy outcome JSON")),
            "{case}: report outcome must come from the run's execution event"
        );
        let (summary_status, summary_envelope) =
            run_actinglab_summary(&actinglab_binary, root.path(), run_id, case);
        assert!(
            summary_status.success(),
            "{case}: formal actinglab summary must exit zero: {summary_envelope}"
        );
        assert_eq!(
            summary_envelope.get("ok").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            summary_envelope.get("command").and_then(Value::as_str),
            Some("run summary")
        );
        assert_eq!(
            summary_envelope.get("data"),
            Some(&summary),
            "{case}: formal CLI must return the same typed Runtime projection"
        );
        let unknown_run_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_run_id()
            .expect("unknown run id")
            .transport();
        let missing = client
            .summarize_run(unknown_run_id)
            .expect_err("unknown run must not produce a success-looking summary");
        assert_eq!(missing.code(), "run_summary_not_found", "{case}");
        let (missing_status, missing_envelope) =
            run_actinglab_summary(&actinglab_binary, root.path(), unknown_run_id, case);
        assert!(
            !missing_status.success(),
            "{case}: unknown typed run must exit non-zero"
        );
        assert_eq!(
            missing_status.code(),
            Some(2),
            "{case}: unknown typed run must retain the usage-validation exit"
        );
        assert_eq!(
            missing_envelope.get("ok").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            missing_envelope
                .pointer("/error/code")
                .and_then(Value::as_str),
            Some("run_summary_not_found")
        );
        assert!(
            missing_envelope.get("data").is_none(),
            "{case}: unknown run must not carry success-style data"
        );
        let capture_count_before = client
            .query_events(
                EventQuery {
                    event_type: Some(EventType::CaptureRequested),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .expect("query fixture captures")
            .len();
        let device_client = connect(root.path());
        let denied = device_client
            .observe_readonly(INSTANCE_ALIAS)
            .expect_err("fixture provider must be isolated from normal device operations");
        assert_eq!(denied.code(), "runtime_request_rejected");
        assert_eq!(
            denied.projection().map(|projection| projection.code),
            Some(RuntimeErrorCode::InvalidRequest)
        );
        drop(device_client);
        let capture_count_after = client
            .query_events(
                EventQuery {
                    event_type: Some(EventType::CaptureRequested),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .expect("query fixture captures after denied device operation")
            .len();
        assert_eq!(capture_count_after, capture_count_before);
        assert!(child.0.try_wait().expect("process state").is_none());

        drop(client);
        child.0.kill().expect("kill actingd");
        child.0.wait().expect("wait actingd");
    }
}

#[test]
fn actingd_scheduled_failure_persists_failed_outcome_completion_and_report() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    write_policy_execution_config(
        &config_path,
        root.path(),
        instance_id(),
        &[
            vec![255, 0, 0, 0, 255, 0],
            vec![255, 0, 0, 0, 255, 0],
            vec![255, 0, 0, 0, 255, 0],
        ],
        2,
    );

    let failed = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .output()
        .expect("run failing scheduled actingd");
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("contained_task_requires_scheduler"),
        "unexpected scheduled failure: {}",
        String::from_utf8_lossy(&failed.stderr)
    );

    let mut recovery_config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("read recovery config"))
            .expect("decode recovery config");
    recovery_config
        .as_object_mut()
        .expect("recovery config object")
        .remove("policy");
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&recovery_config).expect("recovery config JSON"),
    )
    .expect("write recovery config");
    let recovered = start_actingd(&config_path);
    let mut recovered = ChildGuard(recovered);
    wait_for_runtime_info(&mut recovered.0, root.path());
    let client = wait_for_agent_client(&mut recovered.0, root.path());
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("query recovered scheduled failure");
    let intent = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchIntent)
        .expect("failed policy intent");
    let run_id = intent.links.run_id().copied().expect("failed run id");
    let run_events = events
        .iter()
        .filter(|event| event.links.run_id() == Some(&run_id))
        .collect::<Vec<_>>();
    for event_type in [
        EventType::PolicyDispatchIntent,
        EventType::PolicyDispatchAdmitted,
        EventType::LeaseGranted,
        EventType::LabRequest,
        EventType::TaskRequested,
        EventType::TaskFailed,
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            run_events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "failed scheduled {event_type:?}"
        );
    }
    let execution = run_events
        .iter()
        .find(|event| event.event_type == EventType::PolicyExecutionRecorded)
        .expect("failed policy execution");
    let ProjectionPayload::Full(payload) = &execution.payload else {
        panic!("failed forensic execution payload")
    };
    let EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) = payload.as_ref() else {
        panic!("failed policy execution payload")
    };
    let PolicyExecutionOutcome::Failed { failure } = payload.outcome() else {
        panic!("expected failed policy outcome")
    };
    assert_eq!(failure.error_code, "contained_task_requires_scheduler");
    assert_eq!(failure.original_class, PolicyFailureClass::Recoverable);
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );

    let summary = client
        .summarize_run(run_id)
        .expect("summarize failed scheduled run");
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("simulated_failed")
    );
    assert_eq!(
        summary.pointer("/outcome/result").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        summary.get("effect").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        summary.pointer("/outcome/policy"),
        Some(&serde_json::to_value(payload.outcome()).expect("failed outcome JSON"))
    );
    assert_eq!(
        summary
            .get("actual_effect_count")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        summary
            .get("simulated_effect_count")
            .and_then(Value::as_u64),
        Some(2)
    );
    assert!(recovered.0.try_wait().expect("recovery process state").is_none());
    drop(client);
    recovered.0.kill().expect("kill recovery actingd");
    recovered.0.wait().expect("wait recovery actingd");
}

#[test]
fn actingd_summarizes_a_completed_policy_run_across_more_than_one_event_page() {
    const STEP_COUNT: usize = 31;
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let (package, frames) = sequential_contained_task_package(STEP_COUNT);
    let expected_package_digest = format!("sha256:{:x}", Sha256::digest(&package));
    write_policy_execution_config_with_package(
        &config_path,
        root.path(),
        instance_id(),
        &package,
        &frames,
        STEP_COUNT as u16,
    );

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());
    let client = wait_for_agent_client(&mut child.0, root.path());
    let started = Instant::now();
    loop {
        let completed = client
            .query_events(
                EventQuery {
                    event_type: Some(EventType::PolicyDispatchCompleted),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .expect("query policy completion");
        if !completed.is_empty() {
            break;
        }
        if let Some(status) = child.0.try_wait().expect("process state") {
            let mut stderr = String::new();
            if let Some(pipe) = child.0.stderr.as_mut() {
                pipe.read_to_string(&mut stderr)
                    .expect("read actingd stderr");
            }
            panic!("actingd exited before paginated run completed with {status}: {stderr}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "paginated policy run timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let intents = client
        .query_events(
            EventQuery {
                event_type: Some(EventType::PolicyDispatchIntent),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query policy intent");
    let run_id = intents
        .iter()
        .find_map(|event| event.links.run_id().copied())
        .expect("policy run id");
    let run_query = EventQuery {
        run_id: Some(run_id),
        ..EventQuery::default()
    };
    let first_page = client
        .query_event_page(
            run_query.clone(),
            ProjectionProfile::Forensic,
            RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, None)
                .expect("first page request"),
        )
        .expect("first run event page");
    assert_eq!(
        first_page.returned_count(),
        MAX_RUNTIME_EVENT_QUERY_EVENTS,
        "the first page must fill the actual Runtime page limit"
    );
    assert!(
        first_page.has_more(),
        "the first page must expose a continuation"
    );
    let cursor = first_page
        .next_cursor()
        .cloned()
        .expect("first page continuation cursor");
    assert_eq!(
        cursor.snapshot_ledger_position(),
        first_page.snapshot_ledger_position()
    );
    assert_eq!(
        first_page.events().last().map(|event| event.sequence),
        Some(cursor.after_sequence())
    );
    assert!(
        cursor
            .matches(&run_query, ProjectionProfile::Forensic)
            .expect("cursor query identity")
    );
    let second_page = client
        .query_event_page(
            run_query,
            ProjectionProfile::Forensic,
            RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, Some(cursor.clone()))
                .expect("second page request"),
        )
        .expect("second run event page");
    assert_eq!(
        second_page.snapshot_ledger_position(),
        first_page.snapshot_ledger_position(),
        "continuation must stay on the frozen ledger snapshot"
    );
    assert!(
        second_page
            .events()
            .first()
            .is_some_and(|event| event.sequence > cursor.after_sequence()),
        "continuation must advance beyond the first page"
    );
    assert!(second_page.events().iter().all(|second| {
        first_page
            .events()
            .iter()
            .all(|first| first.event_id != second.event_id)
    }));
    let summary = client
        .summarize_run(run_id)
        .expect("summarize all pages of the completed policy run");
    assert!(
        summary
            .get("event_count")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|count| count > u64::from(MAX_RUNTIME_EVENT_QUERY_EVENTS)),
        "the regression must cross the actual Runtime event-page boundary: {summary}"
    );
    assert_eq!(
        summary.get("status").and_then(serde_json::Value::as_str),
        Some("simulated_completed")
    );
    assert_eq!(
        summary
            .get("execution_provenance")
            .and_then(|value| value.get("kind"))
            .and_then(serde_json::Value::as_str),
        Some("fixture_simulation")
    );
    assert_eq!(
        summary
            .get("simulated_effect_count")
            .and_then(serde_json::Value::as_u64),
        Some(STEP_COUNT as u64)
    );
    assert_eq!(
        summary
            .get("actual_effect_count")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert_eq!(
        summary
            .get("package_digest")
            .and_then(serde_json::Value::as_str),
        Some(expected_package_digest.as_str())
    );
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
    write_policy_execution_config(
        &config_path,
        root.path(),
        instance_id(),
        &[vec![0, 0, 255, 0, 255, 0]],
        0,
    );
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

fn write_legacy_physical_policy_config(path: &Path, state_root: &Path, instance_id: InstanceId) {
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

fn write_policy_execution_config(
    path: &Path,
    state_root: &Path,
    instance_id: InstanceId,
    frames: &[Vec<u8>],
    max_inputs: u16,
) {
    write_policy_execution_config_with_package(
        path,
        state_root,
        instance_id,
        &neutral_contained_task_package(),
        frames,
        max_inputs,
    );
}

fn write_policy_execution_config_with_package(
    path: &Path,
    state_root: &Path,
    instance_id: InstanceId,
    package: &[u8],
    frames: &[Vec<u8>],
    max_inputs: u16,
) {
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
    fs::write(policy_root.join("task.zip"), package).expect("write contained package");
    let package_sha256 = Sha256::digest(package)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let fixture_frames = frames
        .iter()
        .map(|rgb| json!({"width": 2, "height": 1, "rgb": rgb}))
        .collect::<Vec<_>>();
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
                "package_digest": format!("sha256:{package_sha256}"),
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {
                    "mode": "fixture_simulation",
                    "package_path": "policy/task.zip"
                }
            }]
        },
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id,
            "fixture_backend": {
                "frames": fixture_frames,
                "max_inputs": max_inputs
            }
        }]
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("policy execution config json"),
    )
    .expect("write policy execution config");
}

fn neutral_contained_task_package() -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{
                "schema_version":"Lab-1y.control.v1",
                "package_id":"neutral.semantic.task",
                "execution_mode":"navigable_route",
                "game":"neutral",
                "server":"test",
                "resolution":{"width":2,"height":1},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":1000,
                "max_steps":2
            }"#,
        ),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{
                "schema_version":"0.6",
                "task_id":"task",
                "game":"neutral",
                "server_scope":["test"],
                "coordinate_space":{"width":2,"height":1},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0},
                    "unguarded_trusted_coordinate":true
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":2,"height":1},
                "defaults":{"color_max_distance":0.0},
                "targets":[
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                ]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (path, contents) in files {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn sequential_contained_task_package(step_count: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
    assert!((1..=31).contains(&step_count));
    let page_names = (0..=step_count)
        .map(|index| format!("page-{index:02}"))
        .collect::<Vec<_>>();
    let operations = (0..step_count)
        .map(|index| {
            json!({
                "id": format!("step_{index:02}"),
                "from": page_names[index],
                "to": page_names[index + 1],
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true
            })
        })
        .collect::<Vec<_>>();
    let recognition_targets = page_names
        .iter()
        .enumerate()
        .map(|(index, page)| {
            json!({
                "type": "color",
                "id": format!("page/{page}"),
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "expected": [u8::try_from(index).expect("page color"), 0, 0]
            })
        })
        .collect::<Vec<_>>();
    let recognition_pages = page_names
        .iter()
        .map(|page| {
            json!({
                "id": format!("neutral/{page}"),
                "required": [format!("page/{page}")],
                "optional": [],
                "forbidden": []
            })
        })
        .collect::<Vec<_>>();
    let documents = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": "neutral.pagination.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 2, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 250,
                "timeout_ms": 10_000,
                "max_steps": step_count + 1
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 2, "height": 1},
                "entry_page": page_names.first().expect("entry page"),
                "target_page": page_names.last().expect("target page"),
                "operations": operations
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": recognition_targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": recognition_pages}),
        ),
    ];
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, document) in documents {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&document).expect("zip JSON"))
            .expect("zip content");
    }
    let frames = (0..=step_count)
        .map(|index| vec![u8::try_from(index).expect("frame color"), 0, 0, 0, 255, 0])
        .collect();
    (zip.finish().expect("finish zip").into_inner(), frames)
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

fn run_actinglab_summary(
    actinglab_binary: &Path,
    state_root: &Path,
    run_id: actingcommand_contract::RunId,
    case: &str,
) -> (std::process::ExitStatus, Value) {
    let run_id = serde_json::to_value(run_id)
        .expect("run id JSON")
        .as_str()
        .expect("run id string")
        .to_owned();
    let output = Command::new(actinglab_binary)
        .args([
            "--json",
            "run",
            "summary",
            &run_id,
            "--state-root",
            state_root.to_str().expect("state root"),
        ])
        .output()
        .unwrap_or_else(|error| panic!("{case}: start actinglab summary: {error}"));
    let envelope = serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{case}: parse actinglab summary JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status, envelope)
}

fn build_candidate_actinglab() -> (TempDir, PathBuf) {
    let target = TempDir::new().expect("fresh actinglab target");
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(workspace_root)
        .args([
            "build",
            "--locked",
            "-p",
            "actingcommand-actinglab",
            "--bin",
            "actinglab",
            "--target-dir",
        ])
        .arg(target.path())
        .output()
        .expect("build exact actinglab candidate");
    assert!(
        output.status.success(),
        "build exact actinglab candidate: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let binary = target.path().join("debug").join(if cfg!(windows) {
        "actinglab.exe"
    } else {
        "actinglab"
    });
    let binary = binary
        .canonicalize()
        .expect("exact actinglab candidate binary");
    let target_root = target.path().canonicalize().expect("actinglab target root");
    assert!(
        binary.starts_with(&target_root),
        "actinglab candidate escaped its fresh target: {}",
        binary.display()
    );
    (target, binary)
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
