// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn scheduled_failure_chain_retries_five_times_and_stops_on_sixth() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_region_retrying_contained_task_package();
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
        panic!("expected one policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("sixth operation attempt must stop the scheduled run");
    assert_eq!(error.code(), "contained_task_requires_scheduler");
    assert!(!error.is_fatal());

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            6,
            "{event_type:?} must record every in-run attempt"
        );
    }
    for event_type in [
        EventType::PolicyDispatchAdmitted,
        EventType::LeaseGranted,
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
            "{event_type:?} must be unique for the formal run"
        );
    }
    let attempts = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEffectIntent)
        .collect::<Vec<_>>();
    let first_attempt = attempts.first().expect("first effect attempt");
    assert!(attempts.iter().all(|attempt| {
        attempt.links.request_id() == first_attempt.links.request_id()
            && attempt.links.correlation_id() == first_attempt.links.correlation_id()
            && attempt.links.task_id() == first_attempt.links.task_id()
            && attempt.links.run_id() == first_attempt.links.run_id()
            && attempt.links.lease_id() == first_attempt.links.lease_id()
    }));
    let mut action_ids = BTreeSet::new();
    let mut action_seeds = BTreeSet::new();
    for attempt in &attempts {
        assert!(
            action_ids.insert(*attempt.links.action_id().expect("effect action id")),
            "StepFinished must close each attempt before the next StepStarted mints an action id"
        );
        let ProjectionPayload::Full(payload) = &attempt.payload else {
            panic!("expected full effect payload")
        };
        let EventPayload::Task(TaskPayload::Semantic(payload)) = payload.as_ref() else {
            panic!("expected task semantic effect payload")
        };
        assert!(matches!(
            payload.fact(),
            TaskSemanticFact::EffectIntent { .. }
        ));
        assert!(
            action_seeds.insert(
                payload
                    .sampling()
                    .expect("sampled retry attempt")
                    .action_seed()
            ),
            "every effect attempt must receive a distinct action seed"
        );
    }
    assert_eq!(action_ids.len(), 6);
    assert_eq!(action_seeds.len(), 6);
    let task_id = context.task_id();
    let run_id = context.run_id();
    let lease_id = context.lease_token().lease_id();
    assert_eq!(first_attempt.links.task_id(), Some(&task_id));
    assert_eq!(first_attempt.links.run_id(), Some(&run_id));
    assert_eq!(first_attempt.links.lease_id(), Some(&lease_id));
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
        .expect("scheduled failure outcome");
    let PolicyExecutionOutcome::Failed { failure } = outcome else {
        panic!("expected one final failed policy outcome")
    };
    assert_eq!(failure.error_code, error.code());
    assert_eq!(failure.original_class, PolicyFailureClass::Recoverable);
    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!(failure.consecutive_same_error, 1);
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 6);
    assert!(
        state.capture_count.load(Ordering::Acquire) >= 12,
        "the guarded operation needs at least one initial capture, one post-effect capture per attempt, and one fresh pre-guard capture before attempts 2-6; bounded postcondition polling may add observations"
    );
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_guarded_retry_stops_after_third_attempt_success() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_retrying_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_inputs
        .store(3, Ordering::Release);
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
        panic!("expected one policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("third guarded attempt must complete the scheduled run");
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("complete successful scheduled policy run");

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            3,
            "{event_type:?} must stop at the successful guarded attempt"
        );
    }
    for event_type in [
        EventType::PolicyDispatchAdmitted,
        EventType::LeaseGranted,
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
            "{event_type:?} must be unique for the successful formal run"
        );
    }
    assert!(
        events
            .iter()
            .all(|event| event.links.run_id() == Some(&context.run_id())),
        "every projected attempt and settlement event must stay on the admitted run"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 3);
    assert!(
        state.capture_count.load(Ordering::Acquire) >= 6,
        "three guarded attempts require at least the initial, post-effect, and fresh retry captures; bounded postcondition polling may add observations"
    );
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_declared_error_page_skips_ordinary_retry() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_error_page_retrying_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .error_capture_after_input
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
        panic!("expected one policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("declared error page must not enter the ordinary retry path");
    assert_eq!(error.code(), "contained_task_requires_scheduler");
    assert!(!error.is_fatal());

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
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
            "{event_type:?} must occur once when the first post-effect page is declared erroneous"
        );
    }
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_non_retryable_destination_observation_is_fail_closed_and_no_destination_remains_compatible()
 {
    for (case, with_destination, error_page, reaches_target, expected_success) in [
        ("target-reached", true, false, true, true),
        ("source-unchanged", true, false, false, false),
        ("declared-error-page", true, true, false, false),
        ("no-destination", false, false, true, true),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = if with_destination {
            neutral_non_retryable_destination_package(error_page)
        } else {
            neutral_contained_task_package()
        };
        let package_path = root.path().join("scheduled-task.zip");
        fs::write(&package_path, &package).expect("write package");
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let state = Arc::new(FakeState::default());
        if reaches_target {
            state
                .transition_capture_after_input
                .store(true, Ordering::Release);
        }
        if error_page {
            state
                .error_capture_after_input
                .store(true, Ordering::Release);
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
        .unwrap_or_else(|error| panic!("{case}: runtime host: {error}"));
        host.activate_policy_catalog(&policy_sources(1))
            .unwrap_or_else(|error| panic!("{case}: activate policy catalog: {error}"));
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{case}: policy admission: {error}"))
        else {
            panic!("{case}: expected one policy run context")
        };
        let request =
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .unwrap_or_else(|error| panic!("{case}: contained task request: {error}"));

        if expected_success {
            let receipt = host
                .run_scheduled_contained_task(&context, &request)
                .unwrap_or_else(|error| panic!("{case}: scheduled run: {error}"));
            assert_eq!(receipt.state(), RuntimeReceiptState::Completed, "{case}");
            host.complete_scheduled_policy_run(&context, &receipt)
                .unwrap_or_else(|error| panic!("{case}: complete scheduled run: {error}"));
        } else {
            let error = host
                .run_scheduled_contained_task(&context, &request)
                .expect_err("non-retryable destination failure must settle once");
            assert_eq!(error.code(), "contained_task_requires_scheduler", "{case}");
            assert!(!error.is_fatal(), "{case}");
        }

        let mut client = TestClient::connect(&host);
        let events = projected_events(
            &mut client,
            EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            },
        );
        for event_type in [
            EventType::TaskEffectIntent,
            EventType::TaskEffectCompleted,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                1,
                "{case}: {event_type:?} must occur exactly once"
            );
        }
        let terminal = if expected_success {
            EventType::TaskCompleted
        } else {
            EventType::TaskFailed
        };
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == terminal)
                .count(),
            1,
            "{case}: scheduled run must produce one final terminal"
        );
        assert!(
            events
                .iter()
                .all(|event| event.links.run_id() == Some(&context.run_id())),
            "{case}: every effect and terminal must remain on the admitted run"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 1, "{case}");
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn scheduled_fresh_retry_observation_target_prevents_second_effect() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_retrying_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_capture
        .store(3, Ordering::Release);
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
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");

    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("fresh target observation must complete the original operation");
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("complete scheduled policy run");

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    for event_type in [
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
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
            "fresh target must not start a second attempt: {event_type:?}"
        );
    }
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 3);
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn scheduled_fresh_retry_observation_error_page_prevents_second_effect() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_error_page_retrying_contained_task_package();
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .error_capture_after_capture
        .store(3, Ordering::Release);
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
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("contained task request");

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("fresh declared error page must not start another operation attempt");
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
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
        EventType::TaskFailed,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == event_type)
                .count(),
            1,
            "fresh error page must not start a second attempt: {event_type:?}"
        );
    }
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 3);
    drop(client);
    host.close().expect("close runtime host");
}
