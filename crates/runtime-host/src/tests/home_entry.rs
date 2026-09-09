// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn entry_preflight_facts(events: &[ProjectedEvent]) -> Vec<&TaskSemanticFact> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if event.event_type == EventType::TaskEntryPreflight =>
                {
                    Some(payload.fact())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// Test class: authorized Defect regression. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
#[test]
fn explicit_home_entry_mismatch_fails_before_target_input() {
    let root = TempDir::new().expect("tempdir");
    let target = explicit_home_contained_task_package("fixture01.target", [0, 0, 255], [255, 0, 0]);
    let target_path = root.path().join("fixture01-target.zip");
    fs::write(&target_path, &target).expect("write target package");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "fixture01.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "fixture01.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(
                target_path.display().to_string(),
                format!("{:x}", Sha256::digest(&target)),
            )
            .expect("target request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let facts = entry_preflight_facts(&events);
    assert!(
        events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .any(|fact| matches!(
                fact,
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    executed_steps: Some(0),
                    ..
                }
            ))
    );
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            matched: false,
            ..
        }
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            failure_code: Some(code),
        } if code == "contained_task_home_recovery_binding_missing"
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskRequested)
            .count(),
        0,
        "the target package must not start before Home"
    );
    drop(client);
    host.close().expect("close runtime host");
}

// Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
#[test]
fn explicit_home_entry_already_home_starts_target_once_without_recovery() {
    let root = TempDir::new().expect("tempdir");
    let target = explicit_home_contained_task_package("fixture01.target", [0, 0, 255], [255, 0, 0]);
    let target_path = root.path().join("fixture01-target.zip");
    fs::write(&target_path, &target).expect("write target package");
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_capture
        .store(1, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "fixture01.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "fixture01.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(
                target_path.display().to_string(),
                format!("{:x}", Sha256::digest(&target)),
            )
            .expect("target request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let facts = entry_preflight_facts(&events);
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryRecoveryDecision { required: false }
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::Started,
            failure_code: None,
        }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskRequested)
            .count(),
        1
    );
    assert!(
        !facts
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }))
    );
    drop(client);
    host.close().expect("close runtime host");
    for leaves_home in [false, true] {
        let root = TempDir::new().unwrap();
        let source =
            explicit_home_contained_task_package("fixture01.target", [255, 0, 0], [0, 0, 255]);
        let mut archive = zip::ZipArchive::new(Cursor::new(source)).unwrap();
        let mut package = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut value: serde_json::Value = serde_json::from_reader(&mut entry).unwrap();
            if name == "resources/operations/task/task.json" {
                value["target_page"] = serde_json::json!("other");
                value["operations"][0]["id"] = serde_json::json!("reach_result");
                value["operations"][0]["from"] = serde_json::json!("home");
                value["operations"][0]["to"] = serde_json::json!("other");
            }
            package.start_file(name, FileOptions::default()).unwrap();
            serde_json::to_writer(&mut package, &value).unwrap();
        }
        let bytes = package.finish().unwrap().into_inner();
        let path = root.path().join("required-home.zip");
        fs::write(&path, &bytes).unwrap();
        let state = Arc::new(FakeState::default());
        if leaves_home {
            state
                .transition_capture_after_capture
                .store(2, Ordering::Release);
        } else {
            state
                .transition_capture_after_input
                .store(true, Ordering::Release);
        }
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "fixture01.instance",
                instance_id(),
                state.clone(),
            )),
        )
        .unwrap();
        let mut client = TestClient::connect(&host);
        client.set_receipt_read_timeout();
        let correlation = client.ids.mint_correlation_id().unwrap();
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "fixture01.instance",
                client.ids.mint_holder_id().unwrap(),
                ContainedTaskRequest::new(
                    path.display().to_string(),
                    format!("{:x}", Sha256::digest(&bytes)),
                )
                .unwrap(),
            ),
        );
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            if leaves_home {
                RuntimeReceiptState::Failed
            } else {
                RuntimeReceiptState::Completed
            }
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            usize::from(!leaves_home)
        );
        assert_eq!(
            state.capture_count.load(Ordering::Acquire),
            if leaves_home { 2 } else { 3 }
        );
        let events = projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
        );
        let facts = entry_preflight_facts(&events);
        assert_eq!(
            facts
                .iter()
                .filter(|fact| matches!(fact, TaskSemanticFact::EntryRecognition { .. }))
                .count(),
            2
        );
        assert!(facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryRecognition { matched: true, .. }
        )));
        assert_eq!(
            facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecognition { matched: false, .. }
            )),
            leaves_home
        );
        assert!(
            !facts
                .iter()
                .any(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. }))
        );
        if leaves_home {
            assert!(facts.iter().any(
                |fact| matches!(fact, TaskSemanticFact::EntryTargetDisposition {
                disposition:TaskEntryTargetDisposition::FailClosed, failure_code:Some(code)
            } if code == "contained_task_home_entry_not_matched")
            ));
        } else {
            assert!(
                matches!(receipt.result(), Some(RuntimeResult::ContainedTaskCompleted {
                outcome:TaskOutcome::Success, executed_steps:1, final_page:Some(page), ..
            }) if page == "fixture01/other")
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::TaskRequested)
                .count(),
            1
        );
        drop(client);
        host.close().unwrap();
    }
}

// Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
#[test]
fn explicit_home_entry_runs_one_bound_recovery_then_starts_target() {
    let root = TempDir::new().expect("tempdir");
    let target = explicit_home_contained_task_package("fixture01.target", [0, 0, 255], [255, 0, 0]);
    let recovery =
        explicit_home_contained_task_package("fixture01.return-home", [0, 0, 255], [255, 0, 0]);
    let target_path = root.path().join("fixture01-target.zip");
    let recovery_path = root.path().join("return-home.zip");
    fs::write(&target_path, &target).expect("write target package");
    fs::write(&recovery_path, &recovery).expect("write recovery package");
    let recovery_sha256 = format!("{:x}", Sha256::digest(&recovery));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "fixture01.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let task_request = ContainedTaskRequest::new(
        target_path.display().to_string(),
        format!("{:x}", Sha256::digest(&target)),
    )
    .and_then(|request| {
        request.with_recovery(ContainedTaskRecoveryBinding::new(
            recovery_path.display().to_string(),
            recovery_sha256.clone(),
        )?)
    })
    .expect("target request with recovery");
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "fixture01.instance",
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            executed_steps: 1,
            ..
        })
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let facts = entry_preflight_facts(&events);
    assert!(
        events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .any(|fact| matches!(
                fact,
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Success,
                    executed_steps: Some(1),
                    ..
                }
            ))
    );
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 }
            if package_sha256 == &recovery_sha256
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryRecoveryCompleted {
            package_sha256,
            final_page,
            executed_steps: 1,
        } if package_sha256 == &recovery_sha256 && final_page == "fixture01/home"
    )));
    assert!(facts.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::PostRecovery,
            matched: true,
            ..
        }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskRequested)
            .count(),
        1,
        "recovery must not create a parallel task owner"
    );
    drop(client);
    host.close().expect("close runtime host");
    let mut archive = zip::ZipArchive::new(Cursor::new(recovery)).unwrap();
    let mut package = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_owned();
        let mut value: serde_json::Value = serde_json::from_reader(&mut entry).unwrap();
        if name == "resources/operations/task/task.json" {
            value["scheduling_outcome"] = serde_json::json!({"mappings":[{
                "outcome_key":"at_home","effect":"no_designated_effect","terminal_pages":["home"]
            }]});
        }
        package.start_file(name, FileOptions::default()).unwrap();
        serde_json::to_writer(&mut package, &value).unwrap();
    }
    let incompatible = package.finish().unwrap().into_inner();
    let hash = format!("{:x}", Sha256::digest(&incompatible));
    let prepared = PreparedContainedTask::load(
        "fixture01.instance",
        &incompatible,
        ExternalExpectedSha256::parse_hex(&hash).unwrap(),
    )
    .unwrap();
    assert!(!prepared.is_entry_recovery_compatible());
    struct UnreachableRecovery;
    impl ContainedTaskRuntime for UnreachableRecovery {
        type Error = &'static str;
        fn capture(&mut self) -> Result<Frame, Self::Error> {
            panic!("incompatible recovery captured")
        }
        fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
            panic!("incompatible recovery input")
        }
        fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            panic!("incompatible recovery started")
        }
    }
    assert!(
        matches!(prepared.run_entry_recovery(&mut UnreachableRecovery),
        Err(ContainedTaskRunError::Task(error)) if error.code() == "contained_task_home_recovery_package_incompatible")
    );
    let root = TempDir::new().unwrap();
    let target_path = root.path().join("target.zip");
    let recovery_path = root.path().join("incompatible.zip");
    fs::write(&target_path, &target).unwrap();
    fs::write(&recovery_path, &incompatible).unwrap();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "fixture01.instance",
            instance_id(),
            state.clone(),
        )),
    )
    .unwrap();
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().unwrap();
    let correlation_id = *correlation.transport();
    let binding = ContainedTaskRequest::new(
        target_path.display().to_string(),
        format!("{:x}", Sha256::digest(&target)),
    )
    .unwrap()
    .with_recovery(
        ContainedTaskRecoveryBinding::new(recovery_path.display().to_string(), hash).unwrap(),
    )
    .unwrap();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "fixture01.instance",
            client.ids.mint_holder_id().unwrap(),
            binding,
        ),
    );
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    assert!(entry_preflight_facts(&events).iter().any(
        |fact| matches!(fact, TaskSemanticFact::EntryTargetDisposition {
        disposition:TaskEntryTargetDisposition::FailClosed, failure_code:Some(code)
    } if code == "contained_task_home_recovery_package_incompatible")
    ));
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == EventType::TaskRequested)
    );
    drop(client);
    host.close().unwrap();
}

// Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
#[test]
fn explicit_home_entry_recovery_failure_and_persistent_non_home_fail_closed() {
    for (case, target_home, recovery_home, recovery_other, expected_code, expected_inputs) in [
        (
            "recovery-failure",
            [0, 0, 255],
            [255, 255, 0],
            [0, 0, 255],
            "contained_task_page_unknown",
            0,
        ),
        (
            "persistent-non-home",
            [255, 255, 0],
            [0, 0, 255],
            [255, 0, 0],
            "contained_task_home_recovery_persistently_non_home",
            1,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let target =
            explicit_home_contained_task_package("fixture01.target", target_home, [255, 0, 0]);
        let recovery = explicit_home_contained_task_package(
            "fixture01.return-home",
            recovery_home,
            recovery_other,
        );
        let target_path = root.path().join("fixture01-target.zip");
        let recovery_path = root.path().join("return-home.zip");
        fs::write(&target_path, &target).expect("write target package");
        fs::write(&recovery_path, &recovery).expect("write recovery package");
        let recovery_sha256 = format!("{:x}", Sha256::digest(&recovery));
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "fixture01.instance",
                instance_id(),
                Arc::clone(&state),
            )),
        )
        .expect("runtime host");
        let mut client = TestClient::connect(&host);
        client.set_receipt_read_timeout();
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let task_request = ContainedTaskRequest::new(
            target_path.display().to_string(),
            format!("{:x}", Sha256::digest(&target)),
        )
        .and_then(|request| {
            request.with_recovery(ContainedTaskRecoveryBinding::new(
                recovery_path.display().to_string(),
                recovery_sha256.clone(),
            )?)
        })
        .expect("target request with recovery");
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "fixture01.instance",
                client.ids.mint_holder_id().expect("holder"),
                task_request,
            ),
        );

        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{case}");
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            expected_inputs,
            "{case}"
        );
        let events = projected_events(
            &mut client,
            EventQuery {
                correlation_id: Some(correlation_id),
                ..EventQuery::default()
            },
        );
        let facts = entry_preflight_facts(&events);
        assert!(
            facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::FailClosed,
                    failure_code: Some(code),
                } if code == expected_code
            )),
            "{case}"
        );
        if case == "recovery-failure" {
            assert!(facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecoveryFailed {
                    package_sha256,
                    failure_code,
                } if package_sha256 == &recovery_sha256 && failure_code == expected_code
            )));
        } else {
            assert!(facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecoveryCompleted {
                    executed_steps: 1,
                    ..
                }
            )));
            assert!(facts.iter().any(|fact| matches!(
                fact,
                TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::PostRecovery,
                    matched: false,
                    ..
                }
            )));
            let terminal = events
                .iter()
                .filter_map(projected_task_semantic_fact)
                .find(|fact| matches!(fact, TaskSemanticFact::TerminalCommitted { .. }))
                .expect("persistent non-Home terminal");
            assert!(matches!(
                terminal,
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    executed_steps: Some(1),
                    ..
                }
            ));
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::TaskRequested)
                .count(),
            0,
            "{case}: target package must not start"
        );
        drop(client);
        host.close().expect("close runtime host");
    }
}

// Test class: specification criterion. Task Contract: https://github.com/HS7097/ActingCommand-Workflow/issues/241#issuecomment-5491623342
#[test]
fn non_home_start_task_preserves_behavior_and_ignores_recovery_binding() {
    let root = TempDir::new().expect("tempdir");
    let target = neutral_non_home_start_contained_task_package();
    let target_path = root.path().join("neutral-target.zip");
    fs::write(&target_path, &target).expect("write neutral package");
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let task_request = ContainedTaskRequest::new(
        target_path.display().to_string(),
        format!("{:x}", Sha256::digest(&target)),
    )
    .and_then(|request| {
        request.with_recovery(ContainedTaskRecoveryBinding::new(
            root.path().join("unused.zip").display().to_string(),
            "9".repeat(64),
        )?)
    })
    .expect("neutral request with unused recovery binding");
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    assert!(entry_preflight_facts(&events).is_empty());
    drop(client);
    host.close().expect("close runtime host");
}
