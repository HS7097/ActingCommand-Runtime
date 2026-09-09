// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn mapped_catalog_requires_the_exact_package_declaration_before_effect() {
    for (case, package, outcome_keys) in [
        (
            "missing-declaration",
            neutral_contained_task_package(),
            vec!["mapped-result", "mapped-result-alternate"],
        ),
        (
            "misspelled-declaration-key",
            neutral_mapped_contained_task_package("mapped-reslut", "designated_effect_completed"),
            vec!["mapped-result", "mapped-result-alternate"],
        ),
        (
            "missing-declared-key",
            neutral_mapped_contained_task_package("mapped-result", "designated_effect_completed"),
            vec![
                "mapped-result",
                "mapped-result-alternate",
                "required-result",
            ],
        ),
        (
            "extra-declared-key",
            neutral_mapped_contained_task_package("mapped-result", "designated_effect_completed"),
            vec!["mapped-result"],
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package_path = root.path().join(format!("{case}.zip"));
        fs::write(&package_path, &package).expect("write mismatched package");
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(
                    mapped_policy_facts("mapped-result", false),
                    policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(
                FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                    .fixture_simulation(),
            ),
        )
        .unwrap_or_else(|error| panic!("{case}: runtime host: {error}"));
        host.activate_policy_catalog(&mapped_policy_sources_with_keys(1, &outcome_keys))
            .unwrap_or_else(|error| panic!("{case}: activate mapped catalog: {error}"));
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &mapped_policy_facts("mapped-result", false),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                71,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("{case}: mapped evaluation: {error}"));
        let evaluation = cycle.evaluation.expect("mapped evaluation result");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == "fixture.observe")
            .unwrap_or_else(|| panic!("{case}: mapped source dispatch"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("mapped source reason chain")
            .clone();
        record_policy_approval(&host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{case}: policy admission: {error}"))
        else {
            panic!("{case}: expected mapped policy context")
        };
        let request =
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .expect("mismatched package request");
        let error = host
            .run_scheduled_contained_task(&context, &request)
            .expect_err("mapped catalog must reject a non-exact package declaration");
        assert_eq!(
            error.code(),
            "policy_run_outcome_declaration_mismatch",
            "{case}"
        );
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0, "{case}");
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn mapped_terminal_page_conflicts_and_error_pages_fail_without_success() {
    for (case, terminal_pages, error_pages, expected_code) in [
        (
            "conflicting-terminal",
            vec!["home"],
            Vec::new(),
            "contained_task_outcome_declaration_incomplete",
        ),
        (
            "declared-error-terminal",
            vec!["error"],
            vec!["error"],
            "contained_task_page_set_invalid",
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = neutral_mapped_contained_task_package_with_pages(
            "mapped-result",
            "designated_effect_completed",
            &terminal_pages,
            &error_pages,
        );
        let package_path = root.path().join(format!("{case}.zip"));
        fs::write(&package_path, &package).expect("write mapped package");
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(
                    mapped_policy_facts("mapped-result", false),
                    policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(
                FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                    .fixture_simulation(),
            ),
        )
        .unwrap_or_else(|error| panic!("{case}: runtime host: {error}"));
        host.activate_policy_catalog(&mapped_policy_sources(1, "mapped-result"))
            .unwrap_or_else(|error| panic!("{case}: activate mapped catalog: {error}"));
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &mapped_policy_facts("mapped-result", false),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                72,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("{case}: mapped evaluation: {error}"));
        let evaluation = cycle.evaluation.expect("mapped evaluation result");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == "fixture.observe")
            .unwrap_or_else(|| panic!("{case}: mapped source dispatch"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("mapped source reason chain")
            .clone();
        record_policy_approval(&host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{case}: policy admission: {error}"))
        else {
            panic!("{case}: expected mapped policy context")
        };
        let request =
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .expect("mapped package request");
        let error = host
            .run_scheduled_contained_task(&context, &request)
            .expect_err("invalid terminal evidence cannot return success");
        assert_eq!(error.code(), expected_code, "{case}");
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0, "{case}");
        assert_eq!(state.input_count.load(Ordering::Acquire), 0, "{case}");
        let mut client = TestClient::connect(&host);
        let events = projected_events(
            &mut client,
            EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            },
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::TaskCompleted)
                .count(),
            0,
            "{case}: invalid evidence cannot commit success"
        );
        assert_eq!(
            events
                .iter()
                .filter_map(projected_task_semantic_fact)
                .filter(|fact| matches!(
                    fact,
                    TaskSemanticFact::TerminalCommitted {
                        scheduling_disposition: Some(_),
                        ..
                    }
                ))
                .count(),
            0,
            "{case}: invalid evidence cannot commit a disposition"
        );
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn duplicate_designated_effects_fail_before_terminal_and_downstream_dispatch() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_mapped_retrying_contained_task_package("mapped-result");
    let package_path = root.path().join("duplicate-designated-effects.zip");
    fs::write(&package_path, &package).expect("write mapped retry package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_inputs
        .store(3, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts("mapped-result", false),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&mapped_policy_sources(1, "mapped-result"))
        .expect("activate mapped catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts("mapped-result", false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            73,
            PolicyTrigger::FactsChanged,
        )
        .expect("mapped evaluation");
    let evaluation = cycle.evaluation.expect("mapped evaluation result");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == "fixture.observe")
        .expect("mapped source dispatch")
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("mapped source reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission")
    else {
        panic!("expected mapped policy context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("mapped retry package request");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("duplicate designated effects cannot commit a disposition");
    assert_eq!(
        error.code(),
        "contained_task_outcome_designated_effect_duplicate"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 3);
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskEffectCompleted)
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        0,
        "duplicate evidence cannot commit a task terminal"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
            .count(),
        1,
        "duplicate evidence cannot dispatch another task"
    );
    let execution_outcomes = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                    Some(payload.outcome())
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(execution_outcomes.len(), 1);
    let PolicyExecutionOutcome::Failed { failure } = execution_outcomes[0] else {
        panic!("duplicate evidence may only record a failed execution outcome")
    };
    assert_eq!(failure.error_code, error.code());
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn mapped_terminal_requires_a_final_page_and_same_run_final_observation() {
    for (case, final_page, expected_code) in [
        (
            "missing-final-page",
            None,
            "contained_task_outcome_terminal_page_missing",
        ),
        (
            "missing-final-observation",
            Some("neutral/terminal".to_owned()),
            "contained_task_outcome_final_observation_missing",
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let host = host_with_state(&root, "neutral.instance", state);
        let mut client = TestClient::connect(&host);
        let (_, token) = client.acquire("neutral.instance");
        let request = client.request(RuntimeOperation::Health);
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let task_id = ids.mint_task_id().expect("task id");
        let run_id = ids.mint_run_id().expect("run id");
        let declaration: SchedulingOutcomeDeclaration = serde_json::from_value(serde_json::json!({
            "mappings": [{
                "outcome_key": "mapped-final-evidence",
                "effect": "no_designated_effect",
                "terminal_pages": ["terminal"]
            }]
        }))
        .expect("mapped terminal declaration");
        let error = host
            .append_mapped_contained_task_terminal_for_test(
                &request,
                &token,
                task_id,
                run_id,
                final_page,
                "neutral".to_owned(),
                declaration,
                None,
            )
            .expect_err("mapped terminal evidence must fail closed");
        assert_eq!(error.0, expected_code, "{case}");
        let events = projected_events(
            &mut client,
            EventQuery {
                task_id: Some(*task_id.transport()),
                run_id: Some(*run_id.transport()),
                ..EventQuery::default()
            },
        );
        assert_eq!(
            events
                .iter()
                .filter_map(projected_task_semantic_fact)
                .filter(|fact| matches!(
                    fact,
                    TaskSemanticFact::TerminalCommitted {
                        scheduling_disposition: Some(_),
                        ..
                    }
                ))
                .count(),
            0,
            "{case}"
        );
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            client.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        drop(client);
        host.close().expect("close runtime host");
    }
}
