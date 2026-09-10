// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn mapped_terminal_disposition_is_single_source_for_effect_no_effect_and_opaque_keys() {
    for (case, outcome_key, effect, performs_effect) in [
        (
            "effect-applied",
            "effect-applied",
            "designated_effect_completed",
            true,
        ),
        (
            "no-designated-effect",
            "no-designated-effect",
            "no_designated_effect",
            false,
        ),
        (
            "opaque-navigation-result",
            "arrived",
            "designated_effect_completed",
            true,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = neutral_mapped_contained_task_package(outcome_key, effect);
        let package_path = root.path().join("scheduled-mapped-task.zip");
        fs::write(&package_path, &package).expect("write mapped package");
        let package_sha256 = format!("{:x}", Sha256::digest(&package));
        let shared_instance_id = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(
            POLICY_NOW_UNIX_MS,
            POLICY_NOW_UNIX_MS,
        ));
        let state = Arc::new(FakeState::default());
        if performs_effect {
            state
                .transition_capture_after_input
                .store(true, Ordering::Release);
        } else {
            state
                .transition_capture_after_capture
                .store(1, Ordering::Release);
        }
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_policy_inputs(PolicyInputSnapshot::new(
                    mapped_policy_facts(outcome_key, false),
                    policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(
                FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    shared_instance_id,
                    Arc::clone(&state),
                )
                .fixture_simulation(),
            ),
        )
        .unwrap_or_else(|error| panic!("{case}: runtime host: {error}"));
        host.activate_policy_catalog(&mapped_policy_sources(1, outcome_key))
            .unwrap_or_else(|error| panic!("{case}: activate catalog: {error}"));
        let pre_terminal_conflict = host
            .evaluate_policy_cycle_with_test_inputs(
                &mapped_policy_facts(outcome_key, true),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                6,
                PolicyTrigger::FactsChanged,
            )
            .expect_err("mapped caller outcome must fail before the first terminal");
        assert_eq!(
            pre_terminal_conflict.code(),
            "policy_outcome_authority_conflict",
            "{case}"
        );
        let initial = host
            .evaluate_policy_cycle_with_test_inputs(
                &mapped_policy_facts(outcome_key, false),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("{case}: initial evaluation: {error}"));
        let evaluation = initial.evaluation.as_ref().expect("initial evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == "fixture.observe")
            .unwrap_or_else(|| {
                panic!(
                    "{case}: initial mapped dispatch missing; got {:?}",
                    evaluation
                        .dispatch_intents
                        .iter()
                        .map(|intent| intent.task_id.as_str())
                        .collect::<Vec<_>>()
                )
            })
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("mapped reason chain")
            .clone();
        record_policy_approval(&host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{case}: policy admission: {error}"))
        else {
            panic!("{case}: expected one mapped run context")
        };
        host.evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts(outcome_key, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 1,
                monotonic_ms: POLICY_NOW_UNIX_MS + 1,
            },
            8,
            PolicyTrigger::FactsChanged,
        )
        .unwrap_or_else(|error| panic!("{case}: clear caller outcome: {error}"));
        let request =
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
                .expect("mapped task request");
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .unwrap_or_else(|error| panic!("{case}: mapped run: {error}"));
        let completion = host
            .complete_scheduled_policy_run(&context, &receipt)
            .unwrap_or_else(|error| panic!("{case}: mapped completion: {error}"));
        let repeated_completion = host
            .complete_scheduled_policy_run(&context, &receipt)
            .unwrap_or_else(|error| panic!("{case}: repeated mapped completion: {error}"));
        assert_eq!(
            repeated_completion, completion,
            "{case}: repeated terminal delivery must reuse one settlement"
        );
        let projection = completion
            .1
            .as_ref()
            .unwrap_or_else(|| panic!("{case}: mapped completion wake missing"));
        assert_eq!(
            projection.outcome().disposition().outcome_key(),
            outcome_key,
            "{case}: wake must carry the exact tagged outcome"
        );
        assert!(
            projection.ledger_position() >= projection.outcome().identity().terminal_sequence(),
            "{case}: wake projection must cover its source terminal"
        );
        let snapshot = host
            .policy_outcome_key_snapshot_for_test()
            .expect("settled snapshot");
        let completed = snapshot
            .completed_runs
            .get(&(
                "fixture.observe".to_owned(),
                POLICY_INSTANCE_ALIAS.to_owned(),
            ))
            .expect("this mapped run");
        assert_eq!(
            completed.activity_window_id,
            context.admission().activity.window_id
        );

        let mut client = TestClient::connect(&host);
        let events = projected_events(
            &mut client,
            EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            },
        );
        let disposition = events
            .iter()
            .find_map(|event| match &event.payload {
                ProjectionPayload::Full(payload) => match payload.as_ref() {
                    EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                        TaskSemanticFact::TerminalCommitted {
                            scheduling_disposition: Some(disposition),
                            ..
                        } => Some(disposition.clone()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or_else(|| panic!("{case}: terminal disposition missing"));
        assert_eq!(disposition.outcome_key(), outcome_key, "{case}");
        assert_eq!(
            matches!(
                disposition.effect(),
                SchedulingEffectEvidence::DesignatedEffectCompleted { .. }
            ),
            performs_effect,
            "{case}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::TaskCompleted)
                .count(),
            1,
            "{case}: disposition must share the single terminal event"
        );
        for event_type in [
            EventType::PolicyDispatchAdmitted,
            EventType::PolicyExecutionRecorded,
            EventType::PolicyDispatchCompleted,
        ] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == event_type)
                    .count(),
                1,
                "{case}: repeated delivery cannot duplicate {event_type:?}"
            );
        }
        let terminal_timestamp = events
            .iter()
            .find(|event| event.event_type == EventType::TaskCompleted)
            .expect("mapped terminal")
            .timestamp_unix_ms;
        let mut followup_facts = mapped_policy_facts(outcome_key, false);
        for fact in &mut followup_facts.facts {
            fact.observed_at_unix_ms = terminal_timestamp;
            fact.expires_at_unix_ms = Some(terminal_timestamp + 900_000);
        }

        let followup_time = context
            .admission()
            .activity
            .next_eligible_unix_ms
            .max(terminal_timestamp + 1_000);
        advance_manual_clock_to(&clock, followup_time);
        let followup = host
            .evaluate_policy_cycle_with_test_inputs(
                &followup_facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: followup_time,
                    monotonic_ms: followup_time,
                },
                9,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("{case}: authoritative evaluation: {error}"));
        assert!(
            followup
                .evaluation
                .expect("followup evaluation")
                .dispatch_intents
                .iter()
                .any(|intent| intent.task_id == "fixture.followup"),
            "{case}: exact-run authoritative outcome did not reach the policy consumer"
        );
        let conflict = host
            .evaluate_policy_cycle_with_test_inputs(
                &mapped_policy_facts(outcome_key, true),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: followup_time + 1,
                    monotonic_ms: followup_time + 1,
                },
                10,
                PolicyTrigger::FactsChanged,
            )
            .expect_err("mapped caller outcome must conflict with ledger authority");
        assert_eq!(
            conflict.code(),
            "policy_outcome_authority_conflict",
            "{case}"
        );
        assert_eq!(
            state.input_count.load(Ordering::Acquire),
            usize::from(performs_effect),
            "{case}"
        );
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));

        let restarted = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_policy_inputs(PolicyInputSnapshot::new(
                    mapped_policy_facts(outcome_key, false),
                    policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(
                FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    shared_instance_id,
                    Arc::clone(&state),
                )
                .fixture_simulation(),
            ),
        )
        .unwrap_or_else(|error| panic!("{case}: restart runtime host: {error}"));
        let replayed = restarted
            .evaluate_policy_cycle_with_test_inputs(
                &followup_facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: followup_time + 2,
                    monotonic_ms: followup_time + 2,
                },
                11,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("{case}: replayed evaluation: {error}"));
        let replayed_followup = replayed
            .evaluation
            .expect("replayed evaluation")
            .dispatch_intents
            .into_iter()
            .find(|intent| intent.task_id == "fixture.followup")
            .unwrap_or_else(|| {
                panic!("{case}: restart did not rebuild the authoritative disposition")
            });
        let repeated = restarted
            .evaluate_policy_cycle_with_test_inputs(
                &followup_facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: followup_time + 2,
                    monotonic_ms: followup_time + 2,
                },
                11,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("{case}: repeated replay evaluation: {error}"));
        let repeated_followups = repeated
            .evaluation
            .expect("repeated replay evaluation")
            .dispatch_intents
            .into_iter()
            .filter(|intent| intent.task_id == "fixture.followup")
            .collect::<Vec<_>>();
        assert_eq!(
            repeated_followups.len(),
            1,
            "{case}: repeated projection duplicated the downstream task"
        );
        assert_eq!(
            repeated_followups[0].decision_id, replayed_followup.decision_id,
            "{case}: identical replay inputs must retain one deterministic decision"
        );
        let mut replay_client = TestClient::connect(&restarted);
        let replay_events = projected_events(
            &mut replay_client,
            EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            },
        );
        let replayed_dispositions = replay_events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .filter_map(|fact| match fact {
                TaskSemanticFact::TerminalCommitted {
                    scheduling_disposition: Some(disposition),
                    ..
                } => Some(disposition),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(replayed_dispositions, vec![&disposition], "{case}");
        assert_eq!(
            replay_events
                .iter()
                .filter(|event| event.event_type == EventType::TaskCompleted)
                .count(),
            1,
            "{case}: replay cannot append another terminal"
        );
        assert_eq!(
            replay_events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
                .count(),
            1,
            "{case}: replay cannot admit the original run again"
        );
        drop(replay_client);
        restarted
            .close()
            .unwrap_or_else(|error| panic!("{case}: close restarted host: {error}"));
    }
}

#[test]
fn latest_failed_mapped_run_clears_the_prior_successful_outcome() {
    let outcome_key = "mapped-current-run";
    let root = TempDir::new().expect("tempdir");
    let (host, state, first_context, request, first_policy_unix_ms, clock) =
        admitted_mapped_run_fixture_with_policy_time(&root, outcome_key);
    let first_receipt = host
        .run_scheduled_contained_task(&first_context, &request)
        .expect("first mapped run");
    host.complete_scheduled_policy_run(&first_context, &first_receipt)
        .expect("first mapped completion");
    let first_snapshot = host
        .policy_outcome_key_snapshot_for_test()
        .expect("first completed-run snapshot");
    let snapshot_key = (
        first_context.catalog_task_id().to_owned(),
        first_context.instance_alias().to_owned(),
    );
    assert_eq!(
        first_snapshot
            .completed_runs
            .get(&snapshot_key)
            .expect("first expected run")
            .run_id,
        first_context.run_id()
    );
    let first_terminal = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(first_context.run_id()),
            event_type: Some(EventType::TaskCompleted),
            ..EventQuery::default()
        })
        .expect("query first mapped terminal")
        .into_iter()
        .next()
        .expect("first mapped terminal");
    assert_eq!(first_terminal.timestamp_unix_ms(), first_policy_unix_ms);

    let second = admit_second_mapped_policy_run(
        outcome_key,
        &host,
        &first_context,
        clock.as_ref(),
        outcome_key,
        12_001,
    );
    state.input_count.store(0, Ordering::Release);
    state
        .transition_capture_after_input
        .store(false, Ordering::Release);
    state.refuse_guard_capture.store(true, Ordering::Release);
    let failure = host
        .run_scheduled_contained_task(&second.second_context, &request)
        .expect_err("second mapped run must fail at its guard");
    assert_eq!(failure.code(), "contained_task_guard_refused");
    let failed_run_events = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(second.second_context.run_id()),
            ..EventQuery::default()
        })
        .expect("query failed mapped run");
    let summaries = failed_run_events
        .iter()
        .filter(|event| event.event_type() == EventType::CaptureSummaryCommitted)
        .collect::<Vec<_>>();
    let [summary_event] = summaries.as_slice() else {
        panic!("failed run must commit one capture summary");
    };
    let terminal_event = failed_run_events
        .iter()
        .find(|event| event.event_type() == EventType::TaskFailed)
        .expect("failed task terminal");
    assert!(summary_event.sequence() < terminal_event.sequence());
    let EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) = summary_event.payload()
    else {
        panic!("typed failed capture summary");
    };
    assert_eq!(
        summary.summary().evidence_completeness(),
        actingcommand_contract::EvidenceCompleteness::Complete,
        "task failure and evidence completeness are independent"
    );
    assert!(
        summary
            .summary()
            .pinned()
            .iter()
            .any(|pin| pin.reason() == PinnedFrameReason::GuardRejection)
    );
    assert!(
        summary
            .summary()
            .pinned()
            .iter()
            .any(|pin| pin.reason() == PinnedFrameReason::Failure)
    );
    assert!(
        summary
            .summary()
            .pinned()
            .iter()
            .any(|pin| pin.reason() == PinnedFrameReason::Terminal)
    );
    let failed_snapshot = host
        .policy_outcome_key_snapshot_for_test()
        .expect("failed completed-run snapshot");
    let expected_failed = failed_snapshot
        .completed_runs
        .get(&snapshot_key)
        .expect("latest failed expected run");
    assert_eq!(expected_failed.run_id, second.second_context.run_id());
    assert!(matches!(
        expected_failed.execution_outcome,
        PolicyExecutionOutcome::Failed { .. }
    ));
    assert_ne!(
        first_snapshot, failed_snapshot,
        "completed-run state change must invalidate the earlier policy snapshot"
    );

    let after_failure = evaluate_mapped_policy_after_outcome(
        outcome_key,
        &second,
        &host,
        clock.as_ref(),
        outcome_key,
        12_002,
        12_003,
    );
    assert!(
        after_failure
            .evaluation
            .expect("post-failure evaluation")
            .dispatch_intents
            .iter()
            .all(|intent| intent.task_id != "fixture.followup"),
        "a newer failed exact run cannot reuse the older successful disposition"
    );
    host.close().expect("close mapped current-run host");
}

#[test]
fn mapped_completion_and_cache_transition_are_atomic_to_policy_snapshots() {
    let outcome_key = "mapped-atomic-snapshot";
    let root = TempDir::new().expect("tempdir");
    let (host, _state, first_context, request, first_policy_unix_ms, clock) =
        admitted_mapped_run_fixture_with_policy_time(&root, outcome_key);
    let first_receipt = host
        .run_scheduled_contained_task(&first_context, &request)
        .expect("first mapped run");
    host.complete_scheduled_policy_run(&first_context, &first_receipt)
        .expect("first mapped completion");
    let first_terminal = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(first_context.run_id()),
            event_type: Some(EventType::TaskCompleted),
            ..EventQuery::default()
        })
        .expect("query first terminal")
        .into_iter()
        .next()
        .expect("first terminal");
    assert_eq!(first_terminal.timestamp_unix_ms(), first_policy_unix_ms);
    let second = admit_second_mapped_policy_run(
        outcome_key,
        &host,
        &first_context,
        clock.as_ref(),
        outcome_key,
        12_101,
    );
    let MappedPolicyTimelineFixture { second_context, .. } = second;
    let second_run_id = second_context.run_id();
    let snapshot_key = (
        second_context.catalog_task_id().to_owned(),
        second_context.instance_alias().to_owned(),
    );
    let host = Arc::new(host);
    let control = host
        .pause_policy_outcome_transition_for_test()
        .expect("install atomic-transition hook");
    let transition_host = Arc::clone(&host);
    let transition = thread::spawn(move || {
        transition_host.complete_scheduled_policy_failure_without_terminal_for_test(&second_context)
    });
    control.wait_until_completion_committed();

    let snapshot_host = Arc::clone(&host);
    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    let snapshot = thread::spawn(move || {
        snapshot_tx
            .send(snapshot_host.policy_outcome_key_snapshot_for_test())
            .expect("send policy snapshot");
    });
    assert!(
        matches!(
            snapshot_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "snapshot must wait while completion state and cache are between their atomic update"
    );

    control.resume();
    transition
        .join()
        .expect("join outcome transition")
        .expect("complete failed mapped transition");
    snapshot.join().expect("join policy snapshot");
    let completed_snapshot = snapshot_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("receive completed policy snapshot")
        .expect("read completed policy snapshot");
    let completed = completed_snapshot
        .completed_runs
        .get(&snapshot_key)
        .expect("failed expected run");
    assert_eq!(completed.run_id, second_run_id);
    assert!(matches!(
        completed.execution_outcome,
        PolicyExecutionOutcome::Failed { .. }
    ));

    let host = Arc::try_unwrap(host).unwrap_or_else(|_| panic!("exclusive runtime host"));
    host.close().expect("close atomic-transition host");
}

#[test]
fn newer_success_replaces_an_older_failed_mapped_run() {
    let outcome_key = "mapped-failure-then-success";
    let root = TempDir::new().expect("tempdir");
    let (host, _state, first_context, request, first_policy_unix_ms, clock) =
        admitted_mapped_run_fixture_with_policy_time(&root, outcome_key);
    host.complete_scheduled_policy_failure_without_terminal_for_test(&first_context)
        .expect("first mapped failure");
    let first_completion = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(first_context.run_id()),
            event_type: Some(EventType::PolicyDispatchCompleted),
            ..EventQuery::default()
        })
        .expect("query first completion")
        .into_iter()
        .next()
        .expect("first completion");
    assert_eq!(first_completion.timestamp_unix_ms(), first_policy_unix_ms);

    let second = admit_second_mapped_policy_run(
        outcome_key,
        &host,
        &first_context,
        clock.as_ref(),
        outcome_key,
        12_201,
    );
    let second_receipt = host
        .run_scheduled_contained_task(&second.second_context, &request)
        .expect("second mapped run");
    host.complete_scheduled_policy_run(&second.second_context, &second_receipt)
        .expect("second mapped completion");
    let second_terminal = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(second.second_context.run_id()),
            event_type: Some(EventType::TaskCompleted),
            ..EventQuery::default()
        })
        .expect("query second terminal")
        .into_iter()
        .next()
        .expect("second terminal");
    assert_eq!(
        second_terminal.timestamp_unix_ms(),
        second.timeline.second_policy_unix_ms
    );
    let snapshot = host
        .policy_outcome_key_snapshot_for_test()
        .expect("success completed-run snapshot");
    let completed = snapshot
        .completed_runs
        .get(&(
            second.second_context.catalog_task_id().to_owned(),
            second.second_context.instance_alias().to_owned(),
        ))
        .expect("latest successful expected run");
    assert_eq!(completed.run_id, second.second_context.run_id());
    assert!(matches!(
        completed.execution_outcome,
        PolicyExecutionOutcome::Succeeded { .. }
    ));

    let cycle = evaluate_mapped_policy_after_outcome(
        outcome_key,
        &second,
        &host,
        clock.as_ref(),
        outcome_key,
        12_202,
        12_203,
    );
    assert!(
        cycle
            .evaluation
            .expect("post-success evaluation")
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == "fixture.followup"),
        "the newer successful exact run must replace the older failed completion"
    );
    host.close().expect("close failure-then-success host");
}

#[test]
fn policy_evaluation_rejects_future_observed_outcome() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("future-outcome runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate future-outcome catalog");
    let mut facts = policy_facts();
    facts.outcomes[0].observed_at_unix_ms = POLICY_NOW_UNIX_MS + 1;
    let error = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            12_203,
            PolicyTrigger::FactsChanged,
        )
        .expect_err("future observed outcome must remain fail-closed");
    assert_eq!(error.code(), "policy_evaluation_rejected");
    host.close().expect("close future-outcome host");
}

#[test]
fn two_declared_opaque_outcomes_drive_existing_any_from_one_terminal_disposition() {
    let effect_key = "opaque-effect-result";
    let no_effect_key = "opaque-no-effect-result";
    let root = TempDir::new().expect("tempdir");
    let package = neutral_two_key_mapped_contained_task_package(effect_key, no_effect_key);
    let package_path = root.path().join("two-key-mapped-task.zip");
    fs::write(&package_path, &package).expect("write two-key mapped package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    let runtime_instance_id = instance_id();
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts(effect_key, false),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                runtime_instance_id,
                Arc::clone(&state),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&mapped_two_key_any_policy_sources(
        1,
        effect_key,
        no_effect_key,
    ))
    .expect("activate two-key mapped catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts(effect_key, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            75,
            PolicyTrigger::FactsChanged,
        )
        .expect("two-key initial evaluation");
    let evaluation = cycle.evaluation.expect("two-key evaluation result");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == "fixture.observe")
        .expect("two-key source dispatch")
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("two-key source reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("two-key policy admission")
    else {
        panic!("expected two-key mapped policy context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("two-key package request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("two-key mapped run");
    let (_, projection) = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("two-key mapped completion");
    let projection = projection.expect("two-key authoritative wake projection");
    assert_eq!(projection.outcome().identity().run_id(), context.run_id());
    assert_eq!(projection.outcome().identity().task_id(), context.task_id());
    assert_eq!(
        projection.outcome().identity().decision_id(),
        context.decision_id()
    );
    assert_eq!(
        projection.outcome().identity().lease_id(),
        context.lease_token().lease_id()
    );
    assert!(projection.ledger_position() >= projection.outcome().identity().terminal_sequence());
    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        },
    );
    let terminal = events
        .iter()
        .find(|event| event.event_type == EventType::TaskCompleted)
        .expect("two-key terminal");
    let completion = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchCompleted)
        .expect("two-key settlement");
    assert!(
        projection.ledger_position() >= completion.sequence,
        "the wake lower bound must cover the durable policy settlement"
    );
    let disposition = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .find_map(|fact| match fact {
            TaskSemanticFact::TerminalCommitted {
                scheduling_disposition: Some(disposition),
                ..
            } => Some(disposition),
            _ => None,
        })
        .expect("two-key disposition");
    assert_eq!(disposition.outcome_key(), effect_key);
    // The existing 60s minimum activity interval also exceeds the 1s task cooldown.
    // Use the admitted sample, and keep the original terminal and fact TTL unchanged.
    let followup_unix_ms = terminal
        .timestamp_unix_ms
        .max(context.admission().activity.next_eligible_unix_ms)
        .checked_add(1)
        .expect("two-key followup time");
    let mut facts = mapped_policy_facts(effect_key, false);
    for fact in &mut facts.facts {
        fact.observed_at_unix_ms = terminal.timestamp_unix_ms;
        fact.expires_at_unix_ms = Some(terminal.timestamp_unix_ms + 900_000);
        assert!(
            fact.expires_at_unix_ms
                .is_some_and(|expires_at| followup_unix_ms < expires_at),
            "two-key followup must retain a fresh stop fact"
        );
    }
    drop(client);
    host.close().expect("close before outcome-driven recovery");
    let reopened = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                runtime_instance_id,
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("reopen mapped outcome host");
    let followup = reopened
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms: followup_unix_ms,
                monotonic_ms: followup_unix_ms,
            },
            76,
            PolicyTrigger::FactsChanged,
        )
        .expect("two-key authoritative recovery evaluation");
    let followup_evaluation = followup.evaluation.expect("two-key followup evaluation");
    assert_eq!(
        followup_evaluation
            .dispatch_intents
            .iter()
            .filter(|intent| intent.task_id == "fixture.followup")
            .count(),
        1,
        "existing Any must consume the one exact-run authoritative key; evaluation={followup_evaluation:?}"
    );
    reopened.close().expect("close recovered runtime host");
}

#[test]
fn mapped_startup_outcome_is_rejected_before_policy_dispatch() {
    let root = TempDir::new().expect("tempdir");
    let package =
        neutral_mapped_contained_task_package("startup-forged", "designated_effect_completed");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts("startup-forged", true),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), state).fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&mapped_policy_sources(1, "startup-forged"))
        .expect("activate mapped catalog");
    let error = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect_err("startup outcome cannot become mapped policy authority");
    assert_eq!(error.code(), "policy_outcome_authority_conflict");
    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
            .count(),
        0
    );
    drop(client);
    host.close().expect("close runtime host");
}

#[test]
fn mapped_consumer_projection_failures_are_typed_and_dispatch_nothing() {
    for case in ["projection-failure", "stale-position"] {
        let root = TempDir::new().expect("tempdir");
        let (host, _state, context, request) = admitted_mapped_run_fixture(&root, case);
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .unwrap_or_else(|error| panic!("{case}: mapped run: {error}"));
        host.complete_scheduled_policy_run(&context, &receipt)
            .unwrap_or_else(|error| panic!("{case}: mapped completion: {error}"));
        let terminal = receipt.terminal().expect("mapped terminal");
        match case {
            "projection-failure" => host
                .fail_next_policy_outcome_projection_for_test()
                .expect("inject projection failure"),
            "stale-position" => host
                .override_next_policy_outcome_projection_position_for_test(
                    terminal.sequence.saturating_sub(1),
                )
                .expect("inject stale projection position"),
            _ => unreachable!(),
        }
        let evaluation_time = unix_ms_now().expect("projection evaluation time");
        let mut facts = mapped_policy_facts(case, false);
        for fact in &mut facts.facts {
            fact.observed_at_unix_ms = evaluation_time;
            fact.expires_at_unix_ms = Some(evaluation_time + 900_000);
        }
        let error = host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: evaluation_time,
                    monotonic_ms: evaluation_time,
                },
                74,
                PolicyTrigger::FactsChanged,
            )
            .expect_err("consumer projection failure cannot return a policy cycle");
        assert_eq!(
            error.code(),
            if case == "projection-failure" {
                "policy_outcome_projection_injected_failure"
            } else {
                "outcome_projection_not_ready"
            },
            "{case}"
        );
        let events = host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            })
            .unwrap_or_else(|error| panic!("{case}: query persisted events: {error}"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == EventType::PolicyDispatchAdmitted)
                .count(),
            1,
            "{case}: failed projection cannot admit a downstream task"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type() == EventType::TaskCompleted)
                .count(),
            1,
            "{case}: failed projection cannot rewrite the terminal"
        );
        let close = host.close();
        if case == "projection-failure" {
            assert!(
                close.is_ok()
                    || close.as_ref().is_err_and(|error| {
                        error.code() == "policy_outcome_projection_injected_failure"
                    })
            );
        } else {
            close.unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
        }
    }
}

#[test]
fn unconsumed_package_outcome_preserves_terminal_and_recovers_without_projection() {
    // Workflow #269 B10 first red: issuecomment-5585703260; SCHEDULED-OUTCOMES-v1.
    // B11 first red: issuecomment-5587376490; SCHEDULING-ELIGIBILITY-v1.
    let outcome_key = "unconsumed-result";
    let root = TempDir::new().expect("tempdir");
    let package = neutral_mapped_contained_task_package(outcome_key, "designated_effect_completed");
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let runtime_instance_id = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let mut sources = budget_policy_sources(1);
    let mut tasks: serde_json::Value = serde_json::from_slice(&sources.tasks.bytes).unwrap();
    tasks["tasks"][0]["loop_budget"]["daily_limit"] = serde_json::json!(1);
    tasks["tasks"][0]["loop_budget"]["window_iteration_limit"] = serde_json::json!(1);
    // #267: retain the budget checks, then exercise stop with recovered window evidence.
    tasks["tasks"][0]["feedback_stop"]["schedule"]["at_ms"] =
        serde_json::json!(POLICY_NOW_UNIX_MS + 600_001);
    let mut followup = tasks["tasks"][0].clone();
    followup["id"] = serde_json::json!("fixture.followup");
    followup["priority"] = serde_json::json!(50);
    followup["trigger"] = serde_json::json!({
        "kind": "dependency_completed", "task_id": "fixture.observe", "terminal_states": ["succeeded"]
    });
    let mut fallback = tasks["tasks"][0].clone();
    fallback["id"] = serde_json::json!("fixture.fallback");
    fallback["priority"] = serde_json::json!(1);
    tasks["tasks"]
        .as_array_mut()
        .unwrap()
        .extend([followup, fallback]);
    sources.tasks.bytes = serde_json::to_vec(&tasks).unwrap();
    let mut activity: serde_json::Value = serde_json::from_slice(&sources.activity.bytes).unwrap();
    activity["profiles"][0]["minimum_interval_ms"] = serde_json::json!(60_000);
    activity["profiles"][0]["maximum_interval_ms"] = serde_json::json!(60_000);
    sources.activity.bytes = serde_json::to_vec(&activity).unwrap();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                runtime_instance_id,
                Arc::clone(&state),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog without outcome references");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission")
    else {
        panic!("expected scheduled context")
    };
    let request = ContainedTaskRequest::new(
        package_path.to_string_lossy().into_owned(),
        format!("{:x}", Sha256::digest(&package)),
    )
    .expect("contained task request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("valid package output without a catalog consumer");
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    let completion = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("complete unconsumed outcome");
    assert!(completion.1.is_none());
    assert_eq!(
        host.complete_scheduled_policy_run(&context, &receipt)
            .expect("replay completion"),
        completion
    );
    assert!(
        host.policy_outcome_key_snapshot_for_test()
            .expect("completed-run snapshot")
            .completed_runs
            .is_empty()
    );
    let query = EventQuery {
        run_id: Some(context.run_id()),
        ..EventQuery::default()
    };
    let events = host
        .query_persisted_events_for_test(query.clone())
        .expect("scheduled run events");
    let terminal = events
        .iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .expect("actual terminal");
    let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal.payload() else {
        panic!("expected task semantic payload")
    };
    let TaskSemanticFact::TerminalCommitted {
        outcome,
        final_page,
        scheduling_disposition: Some(disposition),
        ..
    } = payload.fact()
    else {
        panic!("expected actual scheduling disposition")
    };
    assert_eq!(*outcome, TaskOutcome::Success);
    assert_eq!(final_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(disposition.outcome_key(), outcome_key);
    assert!(matches!(
        disposition.effect(),
        SchedulingEffectEvidence::DesignatedEffectCompleted { .. }
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    clock.advance(1_000);
    let waiting = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("actual activity interval defers all three tasks")
        .evaluation
        .unwrap();
    assert!(waiting.dispatch_intents.is_empty());
    assert_eq!(
        waiting.next_wake_unix_ms,
        Some(context.admission().activity.next_eligible_unix_ms)
    );
    assert!(waiting.decisions.iter().all(|decision| {
        decision
            .reasons
            .iter()
            .any(|reason| reason.code == "policy_activity_interval_active")
    }));
    clock.advance(599_000);
    let next = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("completed execution is an authoritative task snapshot")
        .evaluation
        .expect("next evaluation");
    assert_eq!(next.dispatch_intents.len(), 1);
    assert_eq!(next.dispatch_intents[0].task_id, "fixture.followup");
    assert!(
        next.decisions
            .iter()
            .find(|decision| decision.task_id == "fixture.observe")
            .unwrap()
            .reasons
            .iter()
            .any(|reason| reason.code == "policy_budget_exhausted")
    );
    host.close().expect("close scheduled host");

    let restarted = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                runtime_instance_id,
                Arc::clone(&state),
            )
            .fixture_simulation(),
        ),
    )
    .expect("recover completed task without outcome consumption");
    assert!(
        restarted
            .policy_outcome_key_snapshot_for_test()
            .expect("recovered completed-run snapshot")
            .completed_runs
            .is_empty()
    );
    let recovered = restarted
        .query_persisted_events_for_test(query)
        .expect("recovered run events");
    for event_type in [
        EventType::TaskCompleted,
        EventType::PolicyExecutionRecorded,
        EventType::PolicyDispatchCompleted,
    ] {
        assert_eq!(
            recovered
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            1,
            "recovery must retain one {event_type:?}"
        );
    }
    let recovered_terminal = recovered
        .iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .expect("recovered terminal");
    assert_eq!(recovered_terminal.sequence(), terminal.sequence());
    assert_eq!(recovered_terminal.payload(), terminal.payload());
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let next = restarted
        .evaluate_policy_cycle(PolicyTrigger::Recovery)
        .expect("replay derives task state and budget from the same ledger")
        .evaluation
        .expect("recovered evaluation");
    assert_eq!(next.dispatch_intents.len(), 1);
    let next_intent = &next.dispatch_intents[0];
    assert_eq!(next_intent.task_id, "fixture.followup");
    let next_reasons = next
        .reason_chains
        .iter()
        .find(|chain| chain.id == next_intent.reason_chain_id)
        .unwrap();
    assert!(matches!(
        restarted
            .admit_policy_dispatch(
                next_intent,
                next_reasons,
                &policy_context(&restarted, next_intent)
            )
            .expect("next legal task passes the final admission lock"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    clock.advance(PolicyCadence::default().cooldown_ms);
    let stopped = restarted
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("settled unconsumed result enables feedback after recovery")
        .evaluation
        .expect("feedback evaluation");
    assert!(
        stopped
            .decisions
            .iter()
            .find(|decision| decision.task_id == "fixture.observe")
            .unwrap()
            .reasons
            .iter()
            .any(|reason| reason.code == "feedback_stop_true")
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    restarted.close().expect("close recovered host");
}

#[test]
fn direct_mapped_contained_task_commits_typed_terminal_without_generic_fallback() {
    for (case, performs_effect, expected_outcome_key) in [
        ("claimed-mail", true, "claimed"),
        ("empty-mail", false, "no-op"),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("mapped-task.zip");
        let bytes = neutral_two_key_mapped_contained_task_package("claimed", "no-op");
        fs::write(&package, &bytes).expect("write mapped package");
        let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
        let state = Arc::new(FakeState::default());
        if performs_effect {
            state
                .transition_capture_after_input
                .store(true, Ordering::Release);
        } else {
            state
                .transition_capture_after_capture
                .store(1, Ordering::Release);
        }
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                "neutral.instance",
                instance_id(),
                Arc::clone(&state),
            )),
        )
        .unwrap_or_else(|error| panic!("{case}: runtime host: {error}"));
        let mut client = TestClient::connect(&host);
        let correlation = client.ids.mint_correlation_id().expect("correlation");
        let correlation_id = *correlation.transport();
        let request = client.request_with_correlation(
            correlation,
            RuntimeOperation::run_contained_task(
                "neutral.instance",
                client.ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(package.display().to_string(), expected)
                    .expect("mapped task request"),
            ),
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let receipt = client.send(&request);
            assert_eq!(receipt.state(), RuntimeReceiptState::Completed, "{case}");
            assert!(matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    outcome: TaskOutcome::Success,
                    final_page: Some(page),
                    ..
                }) if page == "neutral/terminal"
            ));
            let events = projected_events(
                &mut client,
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
            );
            let terminals = events
                .iter()
                .filter_map(projected_task_semantic_fact)
                .filter_map(|fact| match fact {
                    TaskSemanticFact::TerminalCommitted {
                        outcome,
                        scheduling_disposition,
                        ..
                    } => Some((*outcome, scheduling_disposition.as_ref())),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [(TaskOutcome::Success, Some(disposition))] = terminals.as_slice() else {
                panic!(
                    "{case}: mapped direct task must commit one typed disposition: {terminals:?}"
                );
            };
            assert_eq!(disposition.outcome_key(), expected_outcome_key, "{case}");
            assert_eq!(
                matches!(
                    disposition.effect(),
                    SchedulingEffectEvidence::DesignatedEffectCompleted { .. }
                ),
                performs_effect,
                "{case}"
            );
            assert_eq!(
                matches!(
                    disposition.effect(),
                    SchedulingEffectEvidence::NoDesignatedEffect
                ),
                !performs_effect,
                "{case}"
            );
            assert_eq!(
                state.input_count.load(Ordering::Acquire),
                usize::from(performs_effect),
                "{case}"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == EventType::TaskCompleted)
                    .count(),
                1,
                "{case}: direct mapped task must commit one terminal"
            );
        }));
        if let Err(original) = result {
            let mut output = [0_u8; 60 * 1024];
            let mut remaining = &mut output[..];
            let formatted = write!(
                remaining,
                "mapped case={case}; task operation={:?}; task request_id_json={:?}; task correlation_id_json={:?}\nHost fatal: {:#?}\nLedger evidence gap: query_persisted_events_for_test materializes all matches before a 32-event limit can be applied; open_evidence opens and checks the whole database before its snapshot budget. No unbounded read or new IPC query attempted.\n",
                request.operation(),
                serde_json::to_string(&request.request_id()),
                serde_json::to_string(&request.correlation_id()),
                host.fatal_error(),
            );
            let used = 60 * 1024 - remaining.len();
            let text = match std::str::from_utf8(&output[..used]) {
                Ok(text) => text,
                Err(error) => std::str::from_utf8(&output[..error.valid_up_to()])
                    .expect("valid diagnostic prefix"),
            };
            eprint!("{text}");
            if formatted.is_err() {
                eprintln!("\nFailure output incomplete: 60-KiB diagnostic limit reached.");
            }
            std::panic::resume_unwind(original);
        }
        drop(client);
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn concurrent_mapped_terminals_commit_exactly_one_disposition() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = Arc::new(host_with_state(&root, "neutral.instance", state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("neutral.instance");
    let request = client.request(RuntimeOperation::Health);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_id = ids.mint_task_id().expect("task id");
    let run_id = ids.mint_run_id().expect("run id");
    let declaration: SchedulingOutcomeDeclaration = serde_json::from_value(serde_json::json!({
        "mappings": [{
            "outcome_key": "mapped-concurrent",
            "effect": "no_designated_effect",
            "terminal_pages": ["terminal"]
        }]
    }))
    .expect("mapped terminal declaration");
    declaration.validate().expect("valid mapped declaration");
    host.append_contained_task_semantic_for_test(
        &request,
        &token,
        task_id,
        run_id,
        TaskSemanticFact::RecognitionCompleted {
            candidate_pages: vec!["neutral/terminal".to_owned()],
            matched_page: Some("neutral/terminal".to_owned()),
            frame_width: 2,
            frame_height: 1,
        },
    )
    .expect("same-run final observation");
    let barrier = Arc::new(Barrier::new(3));

    let attempts = (0..2)
        .map(|_| {
            let host = Arc::clone(&host);
            let token = token.clone();
            let request = request.clone();
            let declaration = declaration.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                host.append_mapped_contained_task_terminal_for_test(
                    &request,
                    &token,
                    task_id,
                    run_id,
                    Some("neutral/terminal".to_owned()),
                    "neutral".to_owned(),
                    declaration,
                    None,
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = attempts
        .into_iter()
        .map(|attempt| attempt.join().expect("terminal attempt thread"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let rejected = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .collect::<Vec<_>>();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].1, RuntimeReceiptState::Denied);
    assert_eq!(rejected[0].2, RuntimeErrorCode::InvalidRequest);
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
                    scheduling_disposition: Some(disposition),
                    ..
                } if disposition.outcome_key() == "mapped-concurrent"
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalRejected { .. }))
            .count(),
        1
    );
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    match Arc::try_unwrap(host) {
        Ok(host) => host.close().expect("close host"),
        Err(_) => panic!("terminal attempt retained the host"),
    }
}

#[test]
fn comparison_selected_outcome_reuses_mapped_terminal_owner_and_conflicts_fail_closed() {
    for (selected, expect_success) in [("comparison-recorded", true), ("different-result", false)] {
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
                "outcome_key": "comparison-recorded",
                "effect": "no_designated_effect",
                "terminal_pages": ["terminal"]
            }]
        }))
        .expect("mapped terminal declaration");
        host.append_contained_task_semantic_for_test(
            &request,
            &token,
            task_id,
            run_id,
            TaskSemanticFact::RecognitionCompleted {
                candidate_pages: vec!["neutral/terminal".to_owned()],
                matched_page: Some("neutral/terminal".to_owned()),
                frame_width: 2,
                frame_height: 1,
            },
        )
        .expect("same-run final observation");

        let result = host.append_mapped_contained_task_terminal_for_test(
            &request,
            &token,
            task_id,
            run_id,
            Some("neutral/terminal".to_owned()),
            "neutral".to_owned(),
            declaration,
            Some(selected.to_owned()),
        );
        if expect_success {
            result.expect("matching comparison result key");
            let events = projected_events(
                &mut client,
                EventQuery {
                    task_id: Some(*task_id.transport()),
                    run_id: Some(*run_id.transport()),
                    ..EventQuery::default()
                },
            );
            assert!(
                events
                    .iter()
                    .filter_map(projected_task_semantic_fact)
                    .any(|fact| matches!(
                        fact,
                        TaskSemanticFact::TerminalCommitted {
                            scheduling_disposition: Some(disposition),
                            ..
                        } if disposition.outcome_key() == "comparison-recorded"
                    ))
            );
        } else {
            let error = result.expect_err("comparison/page outcome conflict");
            assert_eq!(error.0, "contained_task_outcome_comparison_conflict");
        }
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            client.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        drop(client);
        host.close().expect("close runtime host");
    }
}

#[test]
fn unmapped_generic_terminals_preserve_success_failure_and_cancelled_outcomes() {
    for (outcome, final_page, failure_code) in [
        (
            TaskOutcome::Success,
            Some("neutral/terminal".to_owned()),
            None,
        ),
        (TaskOutcome::Failure, None, Some("generic_failure")),
        (TaskOutcome::Cancelled, None, Some("generic_cancelled")),
    ] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let host = host_with_state(&root, "neutral.instance", state);
        let mut client = TestClient::connect(&host);
        let (_, token) = client.acquire("neutral.instance");
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let request = client.request(RuntimeOperation::Health);
        host.append_contained_task_terminal_for_test(
            &request,
            &token,
            ids.mint_task_id().expect("task id"),
            ids.mint_run_id().expect("run id"),
            outcome,
            false,
            final_page,
            0,
            failure_code,
        )
        .unwrap_or_else(|error| panic!("{outcome:?} generic terminal: {error:?}"));

        let events = projected_events(
            &mut client,
            EventQuery {
                lease_id: Some(token.lease_id()),
                ..EventQuery::default()
            },
        );
        let terminals = events
            .iter()
            .filter_map(projected_task_semantic_fact)
            .filter_map(|fact| match fact {
                TaskSemanticFact::TerminalCommitted {
                    outcome,
                    scheduling_disposition,
                    ..
                } => Some((*outcome, scheduling_disposition)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminals, vec![(outcome, &None)]);
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            client.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        drop(client);
        host.close().expect("close host");
    }
}
