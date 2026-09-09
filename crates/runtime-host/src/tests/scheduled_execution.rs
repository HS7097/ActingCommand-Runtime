// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn scheduled_policy_checkpoint_is_exact_thread_bound_one_shot_and_clock_independent() {
    for mismatch in ["run", "task", "correlation", "lease"] {
        let root = TempDir::new().expect("tempdir");
        let outcome_key = format!("checkpoint-mismatch-{mismatch}");
        let (host, _state, context, request) = admitted_mapped_run_fixture(&root, &outcome_key);
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .expect("mismatch scheduled run");
        let ids = IdentifierIssuer::new().expect("mismatch identifiers");
        let identity = ScheduledPolicyCheckpointIdentity::for_context(&context);
        let mismatched = match mismatch {
            "run" => {
                identity.with_run_id(*ids.mint_run_id().expect("mismatched run id").transport())
            }
            "task" => {
                identity.with_task_id(*ids.mint_task_id().expect("mismatched task id").transport())
            }
            "correlation" => identity.with_correlation_id(
                *ids.mint_correlation_id()
                    .expect("mismatched correlation id")
                    .transport(),
            ),
            "lease" => identity.with_lease_id(
                *ids.mint_lease_id()
                    .expect("mismatched lease id")
                    .transport(),
            ),
            _ => unreachable!(),
        };
        let control = host
            .count_scheduled_policy_checkpoint_for_test(mismatched)
            .expect("arm mismatched checkpoint");
        host.complete_scheduled_policy_run(&context, &receipt)
            .expect("mismatched checkpoint must not block settlement");
        assert_eq!(
            control.consumed(),
            0,
            "{mismatch} mismatch consumed the checkpoint"
        );
        host.close().expect("close mismatch host");
    }

    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request, _, clock) =
        admitted_mapped_run_fixture_with_policy_time(&root, "checkpoint-exact-one-shot");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("exact checkpoint scheduled run");
    let control = host
        .count_scheduled_policy_checkpoint_for_test(ScheduledPolicyCheckpointIdentity::for_context(
            &context,
        ))
        .expect("arm exact checkpoint");
    let samples_before_background = clock.samples();
    let sample_deadline = Instant::now() + Duration::from_secs(2);
    while clock.samples() == samples_before_background {
        assert!(
            Instant::now() < sample_deadline,
            "lease sweeper did not sample the runtime clock"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        control.consumed(),
        0,
        "background sweeper/clock sample consumed the exact checkpoint"
    );
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("matching execution consumes checkpoint");
    assert_eq!(control.consumed(), 1, "matching checkpoint consumption");
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("matching completion replay");
    assert_eq!(
        control.consumed(),
        1,
        "completion replay consumed the one-shot checkpoint twice"
    );
    host.close().expect("close exact checkpoint host");

    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request) =
        admitted_mapped_run_fixture(&root, "checkpoint-thread-bound");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("thread-bound scheduled run");
    let control = host
        .count_scheduled_policy_checkpoint_for_test(ScheduledPolicyCheckpointIdentity::for_context(
            &context,
        ))
        .expect("arm thread-bound checkpoint");
    let host = Arc::new(host);
    let execution_host = Arc::clone(&host);
    thread::spawn(move || execution_host.complete_scheduled_policy_run(&context, &receipt))
        .join()
        .expect("join mismatched execution thread")
        .expect("mismatched execution thread still settles");
    assert_eq!(
        control.consumed(),
        0,
        "non-owner execution thread consumed the exact checkpoint"
    );
    Arc::try_unwrap(host)
        .unwrap_or_else(|_| panic!("exclusive thread-bound host"))
        .close()
        .expect("close thread-bound checkpoint host");
}

#[test]
fn scheduled_policy_run_reuses_one_request_receipt_for_one_effecting_run() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_region_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    assert_eq!(
        context.lease_token().owner_epoch(),
        host.runtime_info().owner_epoch(),
        "scheduled context must retain the Runtime fencing epoch validated at admission"
    );
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("scheduled policy run receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    let receipt_value = serde_json::to_value(&receipt).expect("receipt JSON");
    let assert_receipt_error = |value: serde_json::Value, expected_code: &str| {
        let mismatched = serde_json::from_value::<RuntimeReceipt>(value)
            .expect("well-formed mismatched receipt");
        let error = host
            .complete_scheduled_policy_run(&context, &mismatched)
            .expect_err("mismatched receipt must be rejected");
        assert_eq!(error.code(), expected_code);
    };

    let mut missing_terminal_value = receipt_value.clone();
    missing_terminal_value
        .as_object_mut()
        .expect("receipt object")
        .remove("terminal");
    assert_receipt_error(
        missing_terminal_value,
        "policy_run_receipt_terminal_missing",
    );

    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let mut wrong_correlation_value = receipt_value.clone();
    wrong_correlation_value["correlation_id"] = serde_json::to_value(
        issuer
            .mint_correlation_id()
            .expect("correlation id")
            .transport(),
    )
    .expect("correlation id JSON");
    assert_receipt_error(
        wrong_correlation_value,
        "policy_run_receipt_identity_mismatch",
    );

    let mut wrong_run_value = receipt_value.clone();
    wrong_run_value["result"]["run_id"] =
        serde_json::to_value(issuer.mint_run_id().expect("run id").transport())
            .expect("run id JSON");
    assert_receipt_error(wrong_run_value, "policy_run_receipt_identity_mismatch");

    let mut wrong_task_value = receipt_value.clone();
    wrong_task_value["result"]["task_id"] =
        serde_json::to_value(issuer.mint_task_id().expect("task id").transport())
            .expect("task id JSON");
    assert_receipt_error(wrong_task_value, "policy_run_receipt_identity_mismatch");

    let mut wrong_request_value = receipt_value;
    wrong_request_value["request_id"] =
        serde_json::to_value(context.admission_request_id()).expect("request id JSON");
    assert_receipt_error(wrong_request_value, "policy_run_receipt_terminal_mismatch");
    let (_, projection) = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("complete scheduled policy run");
    assert!(
        projection.is_none(),
        "an unmapped terminal must not publish a scheduling-outcome wake"
    );

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskEffectCompleted,
        EventType::TaskCompleted,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "{event_type:?} must be unique"
        );
    }
    let terminal = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .expect("task terminal");
    assert_eq!(terminal.links.request_id(), Some(&receipt.request_id()));
    let context_lease_id = context.lease_token().lease_id();
    let granted = events
        .iter()
        .find(|event| event.event_type == EventType::LeaseGranted)
        .expect("lease grant");
    assert_eq!(
        granted.links.lease_id(),
        Some(&context_lease_id),
        "scheduled events must use the lease carried by the fenced policy context"
    );
    let (persisted_action, sampling) = events
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::EffectIntent { action, .. } => {
                        Some((action, payload.sampling().expect("sampling evidence")))
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .expect("durable sampled effect intent");
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::Xorshift64UniformRectV1
    );
    assert_eq!(sampling.source_regions().len(), 1);
    assert_eq!(sampling.source_regions()[0].width(), 2);
    assert_eq!(
        state
            .input_actions
            .lock()
            .expect("fake input actions lock")
            .as_slice(),
        std::slice::from_ref(persisted_action),
        "the durable sampled action must equal the backend action"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_policy_run_failure_records_terminal_outcome_and_completion() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("scheduled task failure");

    assert!(
        !error.is_fatal(),
        "unexpected fatal scheduled error: {error}"
    );
    assert_eq!(error.code(), "contained_task_requires_scheduler");
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
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
            "{event_type:?} must be unique"
        );
    }
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Task(TaskPayload::Semantic(payload))
                        if matches!(
                            payload.fact(),
                            TaskSemanticFact::TerminalCommitted {
                                outcome: TaskOutcome::Failure,
                                failure_code: Some(code),
                                ..
                            } if code == error.code()
                        )
                )
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if matches!(
                            payload.outcome(),
                            PolicyExecutionOutcome::Failed { failure }
                                if failure.error_code == error.code()
                                    && failure.original_class
                                        == PolicyFailureClass::Recoverable
                        )
                )
        )
    }));
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_recognition_and_guard_failures_settle_on_the_admitted_run() {
    for (case, expected_code) in [
        ("recognition-uncertain", "contained_task_page_unknown"),
        ("guard-refused", "contained_task_guard_refused"),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = if case == "guard-refused" {
            neutral_retrying_contained_task_package()
        } else {
            neutral_contained_task_package()
        };
        let package_path = root.path().join("scheduled-task.zip");
        fs::write(&package_path, &package).expect("write package");
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let state = Arc::new(FakeState::default());
        if case == "recognition-uncertain" {
            state.unknown_capture.store(true, Ordering::Release);
        } else {
            state.refuse_guard_capture.store(true, Ordering::Release);
        }
        let host = RuntimeHost::start(
            config(&root).with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
            Arc::new(
                FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                    .fixture_simulation(),
            ),
        )
        .expect("runtime host");
        host.activate_policy_catalog(&policy_sources(1))
            .expect("activate policy catalog");
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let admission = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .expect("policy admission");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("{case}: expected policy run context")
        };
        let request =
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .expect("contained task request");

        let error = match host.run_scheduled_contained_task(&context, &request) {
            Ok(_) => panic!("{case}: scheduled failure missing"),
            Err(error) => error,
        };
        assert_eq!(error.code(), expected_code, "{case}");
        assert!(!error.is_fatal(), "{case}: unexpected fatal error");

        let mut client = TestClient::connect(&host);
        let events = projected_events(
            &mut client,
            EventQuery {
                run_id: Some(context.run_id()),
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
                "{case}: {event_type:?}"
            );
        }
        let task_failed = events
            .iter()
            .find(|event| event.event_type == EventType::TaskFailed)
            .expect("task failure");
        assert_eq!(task_failed.severity, EventSeverity::Warning, "{case}");
        assert!(matches!(
            &task_failed.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Task(TaskPayload::Semantic(payload))
                        if matches!(
                            payload.fact(),
                            TaskSemanticFact::TerminalCommitted {
                                outcome: TaskOutcome::Failure,
                                failure_code: Some(code),
                                ..
                            } if code == expected_code
                        )
                )
        ));
        let execution = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .expect("policy execution");
        assert!(matches!(
            &execution.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                        if matches!(
                            payload.outcome(),
                            PolicyExecutionOutcome::Failed { failure }
                                if failure.error_code == expected_code
                                    && failure.original_class
                                        == PolicyFailureClass::Recoverable
                        )
                )
        ));
        assert!(
            host.pinned_policy_catalog(&intent.decision_id)
                .expect("catalog pin")
                .is_none(),
            "{case}: catalog pin"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        let diagnostics = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .filter(|event| {
                event.artifacts.iter().any(|artifact| {
                    artifact.kind == ArtifactKind::DiagnosticJson
                        && artifact.redaction_state
                            == actingcommand_contract::ArtifactRedactionState::Pending
                })
            })
            .collect::<Vec<_>>();
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("{case}: one sealed task diagnostic")
        };
        assert!(diagnostic.sequence < task_failed.sequence);
        assert_eq!(diagnostic.links.run_id(), Some(&context.run_id()));
        let bytes = read_projected_verified(root.path(), &diagnostic.artifacts[0]).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let records = document["records"].as_array().unwrap();
        assert_eq!(records.last().unwrap()["data"]["code"], expected_code);
        let pages = records
            .iter()
            .filter(|record| record["kind"] == "page")
            .collect::<Vec<_>>();
        assert!(!pages.is_empty());
        assert!(pages.iter().all(|record| !record["frame_id"].is_null()));
        if case == "guard-refused" {
            let refused = records
                .iter()
                .filter(|record| {
                    record["kind"] == "target" && record["data"]["source"]["phase"] == "guard"
                })
                .collect::<Vec<_>>();
            assert!(!refused.is_empty());
            assert!(
                refused
                    .iter()
                    .all(|record| record["data"]["passed"] == false)
            );
            assert!(
                refused
                    .iter()
                    .all(|record| !record["data"]["color"]["distance"].is_null())
            );
            let elapsed = records
                .iter()
                .filter(|record| record["kind"] == "step_elapsed")
                .collect::<Vec<_>>();
            assert!(!elapsed.is_empty());
            assert!(
                elapsed
                    .iter()
                    .all(|record| record["data"]["completed"] == false)
            );
        } else {
            assert!(
                pages
                    .iter()
                    .all(|record| record["data"]["matched"] == false)
            );
        }
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn scheduled_physical_provider_uses_scheduler_origin_and_original_owner_chain() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");

    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("physical scheduled run");
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("complete physical scheduled run");

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::PolicyDispatchAdmitted,
        EventType::LeaseGranted,
        EventType::CommandReceived,
        EventType::CommandValidated,
        EventType::TaskRequested,
        EventType::InputCommitted,
        EventType::TaskCompleted,
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
            "{event_type:?} must be unique"
        );
    }
    assert!(events.iter().all(|event| !matches!(
        event.event_type,
        EventType::LabRequest | EventType::TaskFailed
    )));
    let scheduled_command = events
        .iter()
        .find(|event| event.event_type == EventType::CommandReceived)
        .expect("scheduled command origin");
    assert_eq!(scheduled_command.origin.source(), EventSource::Scheduler);
    assert_eq!(scheduled_command.origin.module(), OriginModule::Scheduler);
    assert_eq!(scheduled_command.origin.actor(), EventActor::Scheduler);
    let outcome = events
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                    Some(payload.outcome())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("physical policy outcome");
    assert!(matches!(outcome, PolicyExecutionOutcome::Succeeded { .. }));
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_physical_identity_and_package_mismatches_fail_before_io() {
    for case in ["provenance", "instance", "package"] {
        let root = TempDir::new().expect("tempdir");
        let (host, state, context, request, resolved) = admitted_physical_run_fixture(&root);
        let expected_code = match case {
            "provenance" => {
                let instance_id = resolved
                    .lock()
                    .expect("physical resolved instance poisoned")
                    .instance_id();
                *resolved
                    .lock()
                    .expect("physical resolved instance poisoned") =
                    ResolvedExecutionInstance::fixture_simulation(instance_id);
                "runtime_instance_identity_mismatch"
            }
            "instance" => {
                *resolved
                    .lock()
                    .expect("physical resolved instance poisoned") =
                    ResolvedExecutionInstance::new(instance_id(), "127.0.0.1:16384");
                "runtime_instance_identity_mismatch"
            }
            "package" => "policy_run_identity_mismatch",
            _ => unreachable!(),
        };
        let request = if case == "package" {
            ContainedTaskRequest::new(request.package_path(), "0".repeat(64))
                .expect("mismatched physical package request")
        } else {
            request
        };
        let error = host
            .run_scheduled_contained_task(&context, &request)
            .expect_err("physical mismatch must fail closed");
        assert_eq!(error.code(), expected_code, "{case}");
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0, "{case}");
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        let close = host.close();
        assert!(
            close.is_ok()
                || close
                    .as_ref()
                    .is_err_and(|close_error| close_error.code() == expected_code),
            "{case}: unexpected close result {close:?}"
        );
    }
}

#[test]
fn scheduled_expired_lease_is_fenced_and_settled_on_the_original_run() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 2_000,
                ..SchedulerConfig::default()
            })
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");

    let mut client = TestClient::connect(&host);
    clock.advance(2_100);
    let expiry_terminal = host
        .expire_lease_once_for_test(context.lease_token())
        .expect("expire the exact scheduled lease");
    assert_eq!(
        host.expire_lease_once_for_test(context.lease_token())
            .expect("replay the exact scheduled lease expiry"),
        expiry_terminal
    );
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("expired scheduled lease must be fenced");
    assert_eq!(error.code(), "lease_mismatch");
    assert!(!error.is_fatal());
    assert_eq!(
        host.expire_lease_once_for_test(context.lease_token())
            .expect("replay expiry after scheduled settlement"),
        expiry_terminal
    );

    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    let expiry_events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::LeaseExpired),
            instance_id: Some(context.lease_token().instance_id()),
            lease_id: Some(context.lease_token().lease_id()),
            ..EventQuery::default()
        },
    );
    let [expired] = expiry_events.as_slice() else {
        panic!("the exact scheduled lease must have one durable expiry")
    };
    assert_eq!(expired.sequence, expiry_terminal.sequence);
    assert_eq!(expired.event_id, expiry_terminal.event_id);
    assert_eq!(
        expired.links.instance_id(),
        Some(&context.lease_token().instance_id())
    );
    assert_eq!(
        expired.links.lease_id(),
        Some(&context.lease_token().lease_id())
    );
    for event_type in [
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
            "{event_type:?} must settle the fenced run exactly once"
        );
    }
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close runtime host");
}
