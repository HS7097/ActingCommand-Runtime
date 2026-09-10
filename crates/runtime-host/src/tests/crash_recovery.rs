// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn policy_dispatch_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_POLICY_CRASH_ROOT") else {
        return;
    };
    let recovery_case = std::env::var("ACTINGCOMMAND_POLICY_RECOVERY_CASE").ok();
    let outcome_crash = matches!(
        std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref(),
        Ok("after_policy_execution"
            | "after_policy_completion"
            | "fail_policy_execution_append"
            | "after_lease_release_before_policy_execution")
    );
    let instance_bytes = fs::read(Path::new(&root).join("instance.json")).expect("instance bytes");
    let instance_id: InstanceId =
        serde_json::from_slice(&instance_bytes).expect("instance identifier");
    let package = outcome_crash.then(neutral_contained_task_package);
    let package_path = Path::new(&root).join("scheduled-task.zip");
    if let Some(package) = &package {
        fs::write(&package_path, package).expect("scheduled package");
    }
    let mut runtime_config = RuntimeHostConfig::new(&root, b"policy-crash-process-salt")
        .with_governance_capability(TEST_GOVERNANCE_CAPABILITY);
    let lease_expiry_clock = (recovery_case.as_deref() == Some("lease-expired"))
        .then(|| Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)));
    runtime_config = match &package {
        Some(package) => runtime_config.with_procedure_manifest(procedure_manifest_with_primary(
            package,
            vec!["after_observation".to_owned()],
        )),
        None => runtime_config.with_procedure_manifest(procedure_manifest()),
    };
    if let Some(clock) = &lease_expiry_clock {
        runtime_config = runtime_config
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            });
    }
    let state = Arc::new(FakeState::default());
    match recovery_case.as_deref() {
        Some("task-recoverable") => {
            state.fail_capture.store(true, Ordering::Release);
            state
                .transient_capture_failure
                .store(true, Ordering::Release);
        }
        Some("task-severe") => {
            state.fail_capture.store(true, Ordering::Release);
        }
        Some("lease-expired") => {}
        _ if outcome_crash => {
            state
                .transition_capture_after_input
                .store(true, Ordering::Release);
        }
        _ => {}
    }
    let provider = FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id, state);
    let provider = if outcome_crash {
        provider.fixture_simulation()
    } else {
        provider
    };
    let host = RuntimeHost::start(runtime_config, Arc::new(provider)).expect("child runtime host");
    let catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("child catalog activation");
    let (_, intent, reason_chain) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.catalog_hash, catalog.catalog_hash());
    record_policy_approval(&host, &intent);
    if std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_POINT").is_some() {
        fs::write(
            Path::new(&root).join("dispatch-before-crash.json"),
            serde_json::to_vec(&(intent.clone(), reason_chain.clone()))
                .expect("pending dispatch bytes"),
        )
        .expect("pending dispatch marker");
    }
    let admission = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
        .expect("child policy admission");
    if outcome_crash {
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected child policy run context")
        };
        if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref()
            == Ok("after_lease_release_before_policy_execution")
        {
            fs::write(
                Path::new(&root).join("exact-checkpoint-identities.json"),
                serde_json::to_vec(&(
                    context.run_id(),
                    context.task_id(),
                    context.correlation_id(),
                    context.lease_token().lease_id(),
                ))
                .expect("exact checkpoint identity bytes"),
            )
            .expect("exact checkpoint identity file");
            let marker = std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_MARKER")
                .expect("exact checkpoint marker path");
            host.exit_at_scheduled_policy_checkpoint_for_test(&context, PathBuf::from(marker))
                .expect("arm exact lifecycle checkpoint");
        }
        if let Some(clock) = &lease_expiry_clock {
            clock.advance(250);
            host.expire_lease_once_for_test(context.lease_token())
                .expect("expire exact child policy lease");
        }
        let package = package.expect("scheduled package");
        let request = ContainedTaskRequest::new(
            package_path.to_string_lossy().into_owned(),
            format!("{:x}", Sha256::digest(package)),
        )
        .expect("scheduled request");
        match recovery_case.as_deref() {
            Some("task-recoverable" | "task-severe" | "lease-expired") => {
                let error = host
                    .run_scheduled_contained_task(&context, &request)
                    .expect_err("child scheduled failure");
                assert_eq!(error.code(), "ledger_failure");
            }
            _ => {
                let receipt = host
                    .run_scheduled_contained_task(&context, &request)
                    .expect("child scheduled run");
                let error = host
                    .complete_scheduled_policy_run(&context, &receipt)
                    .expect_err("child policy append failure");
                assert_eq!(error.code(), "ledger_failure");
            }
        }
        if std::env::var("ACTINGCOMMAND_POLICY_CRASH_POINT").as_deref()
            == Ok("fail_policy_execution_append")
        {
            assert!(Path::new(&root).join("policy-crash-marker").is_file());
            std::process::exit(87);
        }
        panic!("policy outcome crash barrier did not stop the child");
    }
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    fs::write(
        Path::new(&root).join("admitted-before-crash.json"),
        serde_json::to_vec(&(intent, reason_chain)).expect("admitted dispatch bytes"),
    )
    .expect("admitted dispatch marker");
    fs::write(Path::new(&root).join("child-ready"), b"ready").expect("child marker");
    std::process::exit(0);
}

fn exact_checkpoint_prefix_events(root: &Path, run_id: RunId) -> Vec<PersistedEvent> {
    let artifacts = ArtifactStore::open(root).expect("open prefix artifact store");
    let ledger = GlobalLedger::open_evidence(
        actingcommand_ledger::GlobalLedgerEvidenceConfig::new(root),
        |reference| artifacts.verify_recovery_reference(reference).ok(),
    )
    .expect("open exact checkpoint prefix ledger");
    ledger.query(&EventQuery {
        run_id: Some(run_id),
        ..EventQuery::default()
    })
}

#[test]
fn exact_lifecycle_checkpoint_recovers_scheduled_failure_once() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let marker = root.path().join("exact-lifecycle-checkpoint");
    let stdout_path = root.path().join("exact-lifecycle-child.stdout");
    let stderr_path = root.path().join("exact-lifecycle-child.stderr");
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stdout_path)
        .expect("create exact-lifecycle child stdout");
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stderr_path)
        .expect("create exact-lifecycle child stderr");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::crash_recovery::policy_dispatch_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
        .env(
            "ACTINGCOMMAND_POLICY_CRASH_POINT",
            "after_lease_release_before_policy_execution",
        )
        .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
        .env("ACTINGCOMMAND_POLICY_RECOVERY_CASE", "task-severe")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn exact-lifecycle child");
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll exact-lifecycle child") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed out exact-lifecycle child");
            let _ = child.wait();
            panic!("exact-lifecycle child timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let child_stdout = fs::read_to_string(&stdout_path).expect("read exact-lifecycle child stdout");
    let child_stderr = fs::read_to_string(&stderr_path).expect("read exact-lifecycle child stderr");
    assert_eq!(
        status.code(),
        Some(87),
        "first child-process error: status={status}; stdout={child_stdout:?}; stderr={child_stderr:?}"
    );
    assert_eq!(
        fs::read(&marker).expect("exact-lifecycle marker"),
        b"after-durable-lease-release-before-policy-execution-recorded"
    );
    let (run_id, task_id, correlation_id, lease_id): (RunId, TaskId, CorrelationId, LeaseId) =
        serde_json::from_slice(
            &fs::read(root.path().join("exact-checkpoint-identities.json"))
                .expect("exact checkpoint identity bytes"),
        )
        .expect("exact checkpoint identity JSON");

    let prefix = exact_checkpoint_prefix_events(root.path(), run_id);
    for event_type in [EventType::TaskFailed, EventType::LeaseReleased] {
        assert_eq!(
            prefix
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            1,
            "durable checkpoint prefix {event_type:?} count"
        );
    }
    assert!(
        prefix.iter().all(|event| {
            !matches!(
                event.event_type(),
                EventType::TaskCompleted
                    | EventType::TaskCancelled
                    | EventType::PolicyExecutionRecorded
                    | EventType::PolicyDispatchCompleted
            )
        }),
        "checkpoint prefix crossed its semantic boundary"
    );
    let release = prefix
        .iter()
        .find(|event| event.event_type() == EventType::LeaseReleased)
        .expect("checkpoint release event");
    assert_eq!(release.links().run_id(), Some(&run_id));
    assert_eq!(release.links().task_id(), Some(&task_id));
    assert_eq!(release.links().correlation_id(), Some(&correlation_id));
    assert_eq!(release.links().lease_id(), Some(&lease_id));

    let package = fs::read(root.path().join("scheduled-task.zip")).expect("scheduled package");
    let recovered_state = Arc::new(FakeState::default());
    let recovered = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::clone(&recovered_state),
            )
            .fixture_simulation(),
        ),
    );
    let host = match recovered {
        Ok(host) => host,
        Err(first_error) => panic!(
            "first startup/reconciliation error: {first_error}; child stderr={child_stderr:?}"
        ),
    };
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskFailed,
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "recovered {event_type:?} count"
        );
    }
    let release_index = events
        .iter()
        .position(|event| event.event_type == EventType::LeaseReleased)
        .expect("recovered release");
    let execution_index = events
        .iter()
        .position(|event| event.event_type == EventType::PolicyExecutionRecorded)
        .expect("recovered policy execution");
    let completion_index = events
        .iter()
        .position(|event| event.event_type == EventType::PolicyDispatchCompleted)
        .expect("recovered policy completion");
    assert!(
        release_index < execution_index && execution_index < completion_index,
        "recovered event order"
    );
    for event in &events {
        assert_eq!(event.links.run_id(), Some(&run_id), "run identity");
        assert_eq!(event.links.task_id(), Some(&task_id), "task identity");
        assert_eq!(
            event.links.correlation_id(),
            Some(&correlation_id),
            "correlation identity"
        );
        if matches!(
            event.event_type,
            EventType::LeaseGranted
                | EventType::LeaseReleased
                | EventType::PolicyExecutionRecorded
                | EventType::PolicyDispatchCompleted
        ) {
            assert_eq!(event.links.lease_id(), Some(&lease_id), "lease identity");
        }
    }
    assert!(events.iter().all(|event| {
        !matches!(
            event.event_type,
            EventType::TaskEffectIntent
                | EventType::TaskEffectCompleted
                | EventType::InputIntent
                | EventType::InputCommitted
                | EventType::InputFailed
        )
    }));
    assert_eq!(
        recovered_state.input_count.load(Ordering::Acquire),
        0,
        "restart must not replay a business effect"
    );
    drop(client);
    if let Err(cleanup_error) = host.close() {
        panic!("exact-lifecycle recovery passed; cleanup error appended: {cleanup_error}");
    }
}

#[test]
fn policy_pending_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_POLICY_PENDING_ROOT") else {
        return;
    };
    let instance_bytes =
        fs::read(Path::new(&root).join("pending-instances.json")).expect("instance bytes");
    let (instance_a, instance_b): (InstanceId, InstanceId) =
        serde_json::from_slice(&instance_bytes).expect("instance identifiers");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"policy-pending-process-salt")
            .with_procedure_manifest(procedure_manifest())
            .with_governance_capability(TEST_GOVERNANCE_CAPABILITY),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_a,
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_b,
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("pending child runtime host");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("pending child catalog activation");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("pending child evaluation");
    let evaluation = cycle
        .evaluation
        .as_ref()
        .expect("pending child evaluation data");
    assert_eq!(cycle.pending_dispatch_intents.len(), 2);
    let pairs = cycle
        .pending_dispatch_intents
        .iter()
        .map(|intent| {
            let reason = evaluation
                .reason_chains
                .iter()
                .find(|reason| reason.id == intent.reason_chain_id)
                .expect("pending child reason chain");
            (intent.clone(), reason.clone())
        })
        .collect::<Vec<_>>();
    let admitted = pairs
        .iter()
        .find(|(intent, _)| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .expect("admitted child intent")
        .clone();
    let pending = pairs
        .iter()
        .find(|(intent, _)| intent.instance_id == POLICY_INSTANCE_ALIAS_B)
        .expect("pending child intent")
        .clone();
    record_policy_approval(&host, &admitted.0);
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted.0,
            &admitted.1,
            &policy_context(&host, &admitted.0),
        )
        .expect("pending child admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    fs::write(
        Path::new(&root).join("nonempty-pending-before-crash.json"),
        serde_json::to_vec(&(admitted, pending)).expect("pending crash marker bytes"),
    )
    .expect("pending crash marker");
    std::process::exit(86);
}

#[test]
fn orphaned_policy_admission_is_reconciled_after_real_process_kill() {
    for (point, expected_effect, expected_lease_grants) in [
        ("after_policy_intent", EffectDisposition::NotPerformed, 0),
        ("after_lease_grant", EffectDisposition::Indeterminate, 1),
        ("after_budget_commit", EffectDisposition::Indeterminate, 1),
    ] {
        let root = TempDir::new().expect("tempdir");
        let shared_instance_id = instance_id();
        fs::write(
            root.path().join("instance.json"),
            serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
        )
        .expect("instance file");
        let marker = root.path().join("policy-crash-marker");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::crash_recovery::policy_dispatch_crash_child_process",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
            .env("ACTINGCOMMAND_POLICY_CRASH_POINT", point)
            .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn crash child");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.is_file() {
            assert!(Instant::now() < deadline, "crash marker timeout at {point}");
            assert!(
                child.try_wait().expect("poll crash child").is_none(),
                "crash child exited before {point}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().expect("kill crash child");
        let status = child.wait().expect("wait crash child");
        assert!(!status.success());

        let (intent, reason_chain): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
            &fs::read(root.path().join("dispatch-before-crash.json"))
                .expect("pending dispatch marker"),
        )
        .expect("pending dispatch JSON");
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("recovered runtime host");
        assert!(
            host.pinned_policy_catalog(&intent.decision_id)
                .expect("pinned catalog")
                .is_none()
        );
        assert!(matches!(
            host.admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
                .expect("replay reconciled dispatch"),
            PolicyDispatchAdmission::ReplaySuppressed { .. }
        ));

        let mut client = TestClient::connect(&host);
        let events = projected_events(&mut client, EventQuery::default());
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
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::LeaseGranted)
                .count(),
            expected_lease_grants
        );
        let rejection = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchRejected)
            .expect("reconciled rejection");
        let ProjectionPayload::Full(payload) = &rejection.payload else {
            panic!("expected forensic rejection payload")
        };
        let EventPayload::Policy(PolicyPayload::DispatchRejected(_)) = payload.as_ref() else {
            panic!("expected policy rejection payload")
        };
        assert_eq!(payload.effect_disposition(), Some(expected_effect));
        drop(client);

        thread::sleep(Duration::from_millis(50));
        let (_, next_intent, next_reasons) =
            evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
        record_policy_approval(&host, &next_intent);
        let admission = host
            .admit_policy_dispatch(
                &next_intent,
                &next_reasons,
                &policy_context(&host, &next_intent),
            )
            .expect("post-recovery admission");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected post-recovery grant")
        };
        let admission = context.admission();
        assert_eq!(admission.budget.task_daily_used, 1);
        assert_eq!(admission.budget.activity_window_used, 1);
        host.close().expect("close recovered host");

        let reopened = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("reopen reconciled runtime host");
        assert!(
            reopened
                .pinned_policy_catalog(&intent.decision_id)
                .expect("reopened pinned catalog")
                .is_none()
        );
        let mut client = TestClient::connect(&reopened);
        assert_eq!(
            projected_events(&mut client, EventQuery::default())
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
                .count(),
            1
        );
        drop(client);
        reopened.close().expect("close reopened host");
    }
}

#[test]
fn policy_dispatch_accepts_one_late_outcome_after_process_crash() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::crash_recovery::policy_dispatch_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run crash child");
    assert!(status.success());
    assert!(root.path().join("child-ready").is_file());

    let recovery_clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS + 10_000, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(recovery_clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("recovered runtime host");
    let catalog = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("evaluate the recovered policy at its Runtime clock");
    assert_eq!(cycle.directive.kind, PolicyRecomputeKind::Full);
    assert!(
        cycle.pending_dispatch_intents.is_empty(),
        "recovery must not create another dispatch before the late outcome"
    );
    let (intent, reason_chain): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
        &fs::read(root.path().join("admitted-before-crash.json"))
            .expect("admitted dispatch marker"),
    )
    .expect("admitted dispatch JSON");
    assert_eq!(intent.catalog_hash, catalog.catalog_hash());
    let replay = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
        .expect("replay after crash");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let outcome = host
        .record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("record late outcome after crash");
    assert_eq!(
        host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded,)
            .expect("replay late outcome"),
        outcome
    );
    let PolicyExecutionOutcome::Succeeded { runtime_ms } = &outcome.outcome else {
        panic!("expected successful late outcome")
    };
    assert!(*runtime_ms <= 10_000);
    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
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
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close recovered host");

    let reopened = RuntimeHost::start(
        config(&root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(
            POLICY_NOW_UNIX_MS + 20_000,
            0,
        ))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime after late outcome");
    assert_eq!(
        reopened
            .record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded,)
            .expect("replay late outcome after second restart"),
        outcome
    );
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
            .count(),
        1
    );
    drop(client);
    reopened.close().expect("close replayed late outcome host");
}

#[test]
fn first_policy_execution_append_failure_recovers_each_scheduled_outcome_once() {
    let mut recovered_task_failure_code = None;
    for (case, expected_terminal, expected_class) in [
        ("success", Some(EventType::TaskCompleted), None),
        (
            "task-recoverable",
            Some(EventType::TaskFailed),
            Some(PolicyFailureClass::Recoverable),
        ),
        (
            "task-severe",
            Some(EventType::TaskFailed),
            Some(PolicyFailureClass::Severe),
        ),
        ("lease-expired", None, Some(PolicyFailureClass::Severe)),
    ] {
        let root = TempDir::new().expect("tempdir");
        let shared_instance_id = instance_id();
        fs::write(
            root.path().join("instance.json"),
            serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
        )
        .expect("instance file");
        let marker = root.path().join("policy-crash-marker");
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::crash_recovery::policy_dispatch_crash_child_process",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
            .env(
                "ACTINGCOMMAND_POLICY_CRASH_POINT",
                "fail_policy_execution_append",
            )
            .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
            .env("ACTINGCOMMAND_POLICY_RECOVERY_CASE", case)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap_or_else(|error| panic!("run {case} append-failure child: {error}"));
        assert_eq!(
            status.code(),
            Some(87),
            "{case}: child must stop after the deterministic first execution append failure"
        );
        assert!(marker.is_file(), "{case}: append-failure marker");
        let (intent, _): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
            &fs::read(root.path().join("dispatch-before-crash.json"))
                .expect("outcome dispatch marker"),
        )
        .expect("outcome dispatch JSON");
        let recovered_state = Arc::new(FakeState::default());
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::clone(&recovered_state),
            )),
        )
        .unwrap_or_else(|error| panic!("{case}: recover runtime host: {error}"));
        let mut client = TestClient::connect(&host);
        let events = projected_events(&mut client, EventQuery::default());
        let intent_event = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchIntent)
            .expect("policy intent");
        let lease_granted = events
            .iter()
            .find(|event| event.event_type == EventType::LeaseGranted)
            .expect("lease grant");
        let execution = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .expect("recovered execution");
        let completion = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchCompleted)
            .expect("recovered completion");
        for event in [lease_granted, execution, completion] {
            assert_eq!(
                event.links.request_id(),
                intent_event.links.request_id(),
                "{case}: policy request identity"
            );
            assert_eq!(
                event.links.correlation_id(),
                intent_event.links.correlation_id(),
                "{case}: correlation identity"
            );
            assert_eq!(
                event.links.instance_id(),
                intent_event.links.instance_id(),
                "{case}: instance identity"
            );
            assert_eq!(
                event.links.task_id(),
                intent_event.links.task_id(),
                "{case}: task identity"
            );
            assert_eq!(
                event.links.run_id(),
                intent_event.links.run_id(),
                "{case}: run identity"
            );
            assert_eq!(
                event.links.lease_id(),
                lease_granted.links.lease_id(),
                "{case}: lease identity"
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
                .count(),
            1,
            "{case}: execution count"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
                .count(),
            1,
            "{case}: completion count"
        );
        let capture_summaries = events
            .iter()
            .filter(|event| event.event_type == EventType::CaptureSummaryCommitted)
            .collect::<Vec<_>>();
        if let Some(terminal_type) = expected_terminal {
            let [summary_event] = capture_summaries.as_slice() else {
                panic!("{case}: task terminal must have one capture summary");
            };
            let task_terminal = events
                .iter()
                .find(|event| event.event_type == terminal_type)
                .expect("task terminal");
            assert!(
                summary_event.sequence < task_terminal.sequence,
                "{case}: capture summary must precede the task terminal"
            );
            assert_eq!(
                summary_event.origin.source(),
                EventSource::Runtime,
                "{case}: capture-summary source"
            );
            assert_eq!(
                summary_event.origin.module(),
                OriginModule::CapturePipeline,
                "{case}: capture-summary module"
            );
            assert_eq!(
                summary_event.origin.actor(),
                EventActor::Runtime,
                "{case}: capture-summary actor"
            );
            let ProjectionPayload::Full(payload) = &summary_event.payload else {
                panic!("{case}: full capture summary");
            };
            let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) = payload.as_ref()
            else {
                panic!("{case}: typed capture summary");
            };
            if matches!(case, "task-recoverable" | "task-severe") {
                assert_eq!(
                    summary.summary().evidence_completeness(),
                    actingcommand_contract::EvidenceCompleteness::Failed,
                    "{case}: missing capture evidence is not complete"
                );
                assert_eq!(summary.summary().captured(), 0, "{case}");
                assert_eq!(summary.summary().persisted(), 0, "{case}");
                for reason in [PinnedFrameReason::Failure, PinnedFrameReason::Terminal] {
                    assert!(
                        summary
                            .summary()
                            .pinned()
                            .iter()
                            .any(|pin| pin.reason() == reason
                                && pin.frame_index().is_none()
                                && pin.artifact().is_none()),
                        "{case}: missing {reason:?} evidence must be explicit"
                    );
                }
            }
        } else {
            assert!(
                capture_summaries.is_empty(),
                "{case}: a path without a task terminal cannot invent a capture summary"
            );
        }
        let source = match expected_terminal {
            Some(event_type) => events
                .iter()
                .find(|event| event.event_type == event_type)
                .expect("task terminal"),
            None => events
                .iter()
                .find(|event| {
                    event.event_type == EventType::LeaseReleased
                        && event.links.run_id() == intent_event.links.run_id()
                })
                .expect("failure release"),
        };
        let ProjectionPayload::Full(execution_payload) = &execution.payload else {
            panic!("{case}: full execution payload")
        };
        let EventPayload::Policy(PolicyPayload::ExecutionRecorded(execution_payload)) =
            execution_payload.as_ref()
        else {
            panic!("{case}: policy execution payload")
        };
        assert_eq!(
            execution_payload.observed_at_unix_ms(),
            source.timestamp_unix_ms,
            "{case}: persisted recovery time"
        );
        match (case, execution_payload.outcome()) {
            ("success", PolicyExecutionOutcome::Succeeded { .. }) => {}
            ("task-recoverable" | "task-severe", PolicyExecutionOutcome::Failed { failure }) => {
                assert_eq!(
                    failure.original_class,
                    expected_class.expect("failure class")
                );
                let task_terminal = events
                    .iter()
                    .find(|event| event.event_type == EventType::TaskFailed)
                    .expect("task failure");
                assert_eq!(
                    task_terminal.severity,
                    if case == "task-recoverable" {
                        EventSeverity::Warning
                    } else {
                        EventSeverity::Fatal
                    }
                );
                let ProjectionPayload::Full(payload) = &task_terminal.payload else {
                    panic!("{case}: full task failure payload")
                };
                let EventPayload::Task(TaskPayload::Semantic(payload)) = payload.as_ref() else {
                    panic!("{case}: semantic task failure")
                };
                let TaskSemanticFact::TerminalCommitted {
                    failure_code: Some(code),
                    ..
                } = payload.fact()
                else {
                    panic!("{case}: task failure code")
                };
                assert_eq!(failure.error_code, *code);
                match &recovered_task_failure_code {
                    Some(expected) => assert_eq!(
                        code, expected,
                        "structured severity must not depend on the failure code"
                    ),
                    None => recovered_task_failure_code = Some(code.clone()),
                }
            }
            ("lease-expired", PolicyExecutionOutcome::Failed { failure }) => {
                assert_eq!(failure.error_code, "policy_settlement_interrupted");
                assert_eq!(failure.original_class, PolicyFailureClass::Severe);
                assert_eq!(failure.effective_class, PolicyFailureClass::Severe);
                assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
                assert_eq!(failure.retry_attempt, 0);
                assert_eq!(failure.retry_at_unix_ms, None);
                assert!(!failure.reported_success);
                assert_eq!(failure.runtime_ms, 0);
                assert!(events.iter().all(|event| {
                    !matches!(
                        event.event_type,
                        EventType::TaskCompleted
                            | EventType::TaskFailed
                            | EventType::TaskCancelled
                            | EventType::TaskEffectIntent
                            | EventType::TaskEffectCompleted
                            | EventType::InputIntent
                            | EventType::InputCommitted
                            | EventType::InputFailed
                    )
                }));
            }
            _ => panic!("{case}: unexpected recovered outcome"),
        }
        assert_eq!(
            recovered_state.input_count.load(Ordering::Acquire),
            0,
            "{case}: restart must not replay a business effect"
        );
        assert!(
            host.pinned_policy_catalog(&intent.decision_id)
                .expect("catalog pin")
                .is_none(),
            "{case}: catalog pin"
        );
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close recovered host: {error}"));

        let reopened = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::new(FakeState::default()),
            )),
        )
        .unwrap_or_else(|error| panic!("{case}: second restart: {error}"));
        let mut client = TestClient::connect(&reopened);
        let events = projected_events(&mut client, EventQuery::default());
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
                .count(),
            1,
            "{case}: second-restart execution count"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
                .count(),
            1,
            "{case}: second-restart completion count"
        );
        drop(client);
        reopened
            .close()
            .unwrap_or_else(|error| panic!("{case}: close second restart: {error}"));
    }
}

#[test]
fn split_policy_outcome_append_boundaries_recover_completion_exactly_once() {
    {
        let root = TempDir::new().expect("tempdir");
        let (host, _state, context, request, _policy_time, _clock) =
            admitted_mapped_run_fixture_with_policy_time(&root, "resident-reconciled");
        let query = EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        };
        let pending = host
            .query_persisted_events_for_test(query.clone())
            .expect("original pending run facts");
        host.evaluate_policy_cycle(PolicyTrigger::Reconciliation)
            .expect("reconciliation while the exact lease is active");
        assert_eq!(
            host.query_persisted_events_for_test(query.clone())
                .expect("unchanged pending run facts"),
            pending
        );
        assert!(
            host.pinned_policy_catalog(context.decision_id())
                .expect("pending catalog pin")
                .is_some()
        );
        assert!(
            host.query_persisted_events_for_test(query.clone())
                .expect("active run facts")
                .iter()
                .all(|event| !matches!(
                    event.event_type(),
                    EventType::PolicyExecutionRecorded | EventType::PolicyDispatchCompleted
                )),
            "an active admitted run must remain pending"
        );
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .expect("one mapped contained run");
        host.evaluate_policy_cycle(PolicyTrigger::Reconciliation)
            .expect("online settlement from the exact terminal and release");
        let settled = host
            .query_persisted_events_for_test(query.clone())
            .expect("online settled run facts");
        let release = settled
            .iter()
            .find(|event| event.event_type() == EventType::LeaseReleased)
            .expect("contained request release");
        assert_eq!(release.links().request_id(), Some(&receipt.request_id()));
        assert_ne!(
            release.links().request_id(),
            Some(&context.admission_request_id())
        );
        for event_type in [
            EventType::TaskCompleted,
            EventType::LeaseReleased,
            EventType::InputCommitted,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                settled
                    .iter()
                    .filter(|event| event.event_type() == event_type)
                    .count(),
                1,
                "online {event_type:?} must be unique"
            );
        }
        assert!(
            host.pinned_policy_catalog(context.decision_id())
                .expect("online catalog pin")
                .is_none()
        );
        let replay = host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("late same-run completion replays the settled outcome");
        assert!(
            replay.1.is_some(),
            "the mapped outcome is available after online settlement"
        );
        host.evaluate_policy_cycle(PolicyTrigger::Reconciliation)
            .expect("repeated online reconciliation");
        assert_eq!(
            host.query_persisted_events_for_test(query)
                .expect("same run after late completion and reconciliation"),
            settled,
            "online recovery and replay must not append duplicate run facts or effects"
        );
        host.close().expect("close online-reconciled host");
    }
    for point in ["after_policy_execution", "after_policy_completion"] {
        let root = TempDir::new().expect("tempdir");
        let shared_instance_id = instance_id();
        fs::write(
            root.path().join("instance.json"),
            serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
        )
        .expect("instance file");
        let marker = root.path().join("policy-crash-marker");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::crash_recovery::policy_dispatch_crash_child_process",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
            .env("ACTINGCOMMAND_POLICY_CRASH_POINT", point)
            .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn outcome crash child");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.is_file() {
            assert!(
                Instant::now() < deadline,
                "outcome crash marker timeout at {point}"
            );
            assert!(
                child
                    .try_wait()
                    .expect("poll outcome crash child")
                    .is_none(),
                "outcome crash child exited before {point}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().expect("kill outcome crash child");
        let status = child.wait().expect("wait outcome crash child");
        assert!(!status.success());

        let (intent, _): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
            &fs::read(root.path().join("dispatch-before-crash.json"))
                .expect("outcome dispatch marker"),
        )
        .expect("outcome dispatch JSON");
        for restart in 1..=2 {
            let host = RuntimeHost::start(
                config(&root),
                Arc::new(FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    shared_instance_id,
                    Arc::new(FakeState::default()),
                )),
            )
            .unwrap_or_else(|error| panic!("restart {restart} after {point} failed: {error}"));
            let mut client = TestClient::connect(&host);
            let events = projected_events(&mut client, EventQuery::default());
            let intent_event = events
                .iter()
                .find(|event| event.event_type == EventType::PolicyDispatchIntent)
                .expect("original policy intent");
            let original_run_id = intent_event
                .links
                .run_id()
                .copied()
                .expect("original run id");
            let original_request_id = intent_event
                .links
                .request_id()
                .copied()
                .expect("original request id");
            let original_correlation_id = intent_event
                .links
                .correlation_id()
                .copied()
                .expect("original correlation id");
            let original_instance_id = intent_event
                .links
                .instance_id()
                .copied()
                .expect("original instance id");
            let original_task_id = intent_event
                .links
                .task_id()
                .copied()
                .expect("original task id");
            let lease_granted = events
                .iter()
                .find(|event| event.event_type == EventType::LeaseGranted)
                .expect("original lease grant");
            let original_lease_id = lease_granted
                .links
                .lease_id()
                .copied()
                .expect("original lease id");
            let execution = events
                .iter()
                .find(|event| event.event_type == EventType::PolicyExecutionRecorded)
                .expect("policy execution");
            let completion = events
                .iter()
                .find(|event| event.event_type == EventType::PolicyDispatchCompleted)
                .expect("recovered policy completion");
            for event in [lease_granted, execution, completion] {
                assert_eq!(
                    event.links.request_id(),
                    Some(&original_request_id),
                    "restart {restart} after {point}: request identity"
                );
                assert_eq!(
                    event.links.correlation_id(),
                    Some(&original_correlation_id),
                    "restart {restart} after {point}: correlation identity"
                );
                assert_eq!(
                    event.links.instance_id(),
                    Some(&original_instance_id),
                    "restart {restart} after {point}: instance identity"
                );
                assert_eq!(
                    event.links.task_id(),
                    Some(&original_task_id),
                    "restart {restart} after {point}: task identity"
                );
                assert_eq!(
                    event.links.run_id(),
                    Some(&original_run_id),
                    "restart {restart} after {point}: run identity"
                );
                assert_eq!(
                    event.links.lease_id(),
                    Some(&original_lease_id),
                    "restart {restart} after {point}: lease identity"
                );
            }
            assert_ne!(
                execution.links.action_id(),
                completion.links.action_id(),
                "restart {restart} after {point}: recovery must mint a fresh action"
            );
            let original_run_events = projected_events(
                &mut client,
                EventQuery {
                    run_id: Some(original_run_id),
                    ..EventQuery::default()
                },
            );
            assert_eq!(
                original_run_events
                    .iter()
                    .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
                    .count(),
                1,
                "restart {restart} after {point}: execution count"
            );
            assert_eq!(
                original_run_events
                    .iter()
                    .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
                    .count(),
                1,
                "restart {restart} after {point}: completion count"
            );
            assert!(
                host.pinned_policy_catalog(&intent.decision_id)
                    .expect("recovered catalog pin")
                    .is_none(),
                "restart {restart} after {point}: catalog pin retained"
            );
            drop(client);
            host.close().expect("close recovered outcome host");
        }
    }
}

#[test]
fn nonempty_pending_policy_work_is_rebuilt_after_real_process_crash() {
    let root = TempDir::new().expect("tempdir");
    let instance_a = instance_id();
    let instance_b = instance_id();
    fs::write(
        root.path().join("pending-instances.json"),
        serde_json::to_vec(&(instance_a, instance_b)).expect("pending instance bytes"),
    )
    .expect("pending instance file");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::crash_recovery::policy_pending_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_POLICY_PENDING_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run pending crash child");
    assert!(!status.success());

    let (admitted, pending): (
        (DispatchIntent, DecisionReasonChain),
        (DispatchIntent, DecisionReasonChain),
    ) = serde_json::from_slice(
        &fs::read(root.path().join("nonempty-pending-before-crash.json"))
            .expect("nonempty pending marker"),
    )
    .expect("nonempty pending marker JSON");
    assert_ne!(admitted.0.instance_id, pending.0.instance_id);

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_a,
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_b,
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("recovered pending runtime host");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 60_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 60_000,
            },
            8,
            PolicyTrigger::Recovery,
        )
        .expect("rebuild pending policy work");
    assert!(!cycle.pending_dispatch_intents.is_empty());
    assert!(cycle.pending_dispatch_intents.iter().any(|intent| {
        intent.task_id == pending.0.task_id && intent.instance_id == pending.0.instance_id
    }));
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted.0,
            &admitted.1,
            &policy_context(&host, &admitted.0),
        )
        .expect("replay admitted work after crash"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close recovered pending runtime");
}
