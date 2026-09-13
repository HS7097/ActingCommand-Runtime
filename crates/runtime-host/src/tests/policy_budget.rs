// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn policy_budget_recovery_keeps_the_window_count_across_runtime_restarts() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();

    for index in 0_u64..4 {
        let clock = Arc::new(ManualRuntimeClock::new(
            POLICY_NOW_UNIX_MS + index * 600_000,
            0,
        ));
        let host = RuntimeHost::start(
            config(&root).with_runtime_clock(clock.clone()),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("budget runtime host");
        if index == 0 {
            host.activate_policy_catalog(&budget_policy_sources(1))
                .expect("activate budget catalog");
        }
        let (_, intent, reasons) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Recovery,
            POLICY_NOW_UNIX_MS + index * 600_000,
            100 + index,
        );
        if index == 0 {
            record_policy_approval(&host, &intent);
        }
        let admission = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .expect("budget admission");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected budget admission")
        };
        let admission = context.admission();
        assert_eq!(admission.budget.task_window_used, index as u32 + 1);
        clock.advance(75_000);
        host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
            .expect("record bounded execution");
        host.close().expect("close budget runtime host");
    }

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen exhausted budget runtime");
    let evaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 2_400_000,
                monotonic_ms: 2_400_000,
            },
            104,
            PolicyTrigger::Recovery,
        )
        .expect("recovered budget availability")
        .evaluation
        .unwrap();
    assert!(
        evaluation.dispatch_intents.is_empty(),
        "fifth window dispatch cannot win after restart"
    );
    assert!(
        evaluation.decisions[0]
            .reasons
            .iter()
            .any(|reason| reason.code == "policy_budget_exhausted")
    );
    assert!(
        evaluation
            .next_wake_unix_ms
            .is_some_and(|next| next > POLICY_NOW_UNIX_MS + 2_400_000)
    );
    host.close().expect("close exhausted budget runtime");
}

#[test]
fn policy_completion_charges_runtime_owned_monotonic_elapsed_time() {
    for (case, completion_unix_ms, elapsed_ms) in [
        ("zero", POLICY_NOW_UNIX_MS, 0),
        ("wall-clock-rollback", 1, 125),
        ("wall-clock-future", u64::MAX, 250),
    ] {
        let root = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 1_000));
        let bytes = neutral_contained_task_package();
        let package = root.path().join("neutral-step.zip");
        fs::write(&package, &bytes).unwrap();
        let task = ContainedTaskRequest::new(
            package.display().to_string(),
            format!("{:x}", Sha256::digest(&bytes)),
        )
        .unwrap();
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        state.block_input.store(true, Ordering::Release);
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &bytes,
                    vec!["after_observation".into()],
                )),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                state.clone(),
            )),
        )
        .unwrap_or_else(|error| panic!("{case}: start runtime host: {error}"));
        host.activate_policy_catalog(&policy_sources(1))
            .unwrap_or_else(|error| panic!("{case}: activate policy catalog: {error}"));
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let admission = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .unwrap_or_else(|error| panic!("{case}: policy admission: {error}"));
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("{case}: expected granted policy admission")
        };
        let admission = context.admission();

        thread::scope(|scope| {
            let run = scope.spawn(|| host.run_scheduled_contained_task(&context, &task));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !state.input_started.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    state.block_input.store(false, Ordering::Release);
                    panic!("{case}: physical input did not start");
                }
                thread::sleep(Duration::from_millis(1));
            }
            clock.set_unix_ms(completion_unix_ms);
            clock.set_monotonic_ms(1_000 + elapsed_ms);
            let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
            sample_capacity().expect("capacity sample at completion time");
            state.block_input.store(false, Ordering::Release);
            run.join().unwrap().expect("scheduled task completed");
        });
        let mut client = TestClient::connect(&host);
        let events = projected_events(
            &mut client,
            EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            },
        );
        let artifact = events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .flat_map(|event| &event.artifacts)
            .find(|artifact| {
                artifact.kind == ArtifactKind::DiagnosticJson
                    && artifact.redaction_state
                        == actingcommand_contract::ArtifactRedactionState::Pending
            })
            .unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&read_projected_verified(root.path(), artifact).unwrap())
                .unwrap();
        let elapsed = document["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["kind"] == "step_elapsed")
            .collect::<Vec<_>>();
        assert_eq!(elapsed.len(), 1);
        assert_eq!(elapsed[0]["data"]["started_monotonic_ms"], 0);
        assert_eq!(elapsed[0]["data"]["ended_monotonic_ms"], elapsed_ms);
        assert_eq!(elapsed[0]["data"]["elapsed_ms"], elapsed_ms);
        assert_eq!(elapsed[0]["data"]["completed"], true);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::TaskStepFinished)
                .count(),
            1
        );
        drop(client);
        let outcome = host
            .record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
            .unwrap_or_else(|error| panic!("{case}: record policy outcome: {error}"));
        assert_eq!(
            outcome.observed_at_unix_ms,
            admission.activity.admitted_at_unix_ms + elapsed_ms,
            "{case}: wall-clock value must not control the authoritative completion time"
        );
        assert_eq!(
            outcome.outcome,
            PolicyExecutionOutcome::Succeeded {
                runtime_ms: elapsed_ms
            },
            "{case}: budget charge must use Runtime monotonic elapsed time"
        );
        host.close()
            .unwrap_or_else(|error| panic!("{case}: close runtime host: {error}"));
    }
}

#[test]
fn policy_completion_rejects_runtime_monotonic_clock_regression() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 1_000));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");

    clock.set_monotonic_ms(999);
    let error = host
        .record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect_err("monotonic regression must fail loudly");
    assert!(error.is_fatal());
    assert_eq!(error.code(), "monotonic_clock_regressed");
    let close_error = host
        .close()
        .expect_err("fatal clock regression must poison the runtime");
    assert_eq!(close_error.code(), "monotonic_clock_regressed");
}

#[test]
fn accelerated_48h_replay_consumes_runtime_owned_counts_and_runtime_budget() {
    const HOUR_MS: u64 = 3_600_000;
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let start_unix_ms = POLICY_NOW_UNIX_MS + 12 * HOUR_MS;

    for day in 0_u64..2 {
        for iteration in 0_u64..4 {
            let unix_ms = start_unix_ms + (day * 24 + iteration * 6) * HOUR_MS;
            let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
            let host = RuntimeHost::start(
                config(&root).with_runtime_clock(clock.clone()),
                Arc::new(FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    registered_id,
                    Arc::new(FakeState::default()),
                )),
            )
            .expect("accelerated budget runtime");
            if day == 0 && iteration == 0 {
                host.activate_policy_catalog(&budget_policy_sources(1))
                    .expect("activate accelerated budget catalog");
            }
            let (_, intent, reasons) = evaluated_policy_dispatch_at(
                &host,
                if day == 0 && iteration == 0 {
                    PolicyTrigger::FactsChanged
                } else {
                    PolicyTrigger::Reconciliation
                },
                unix_ms,
                200 + day * 10 + iteration,
            );
            if day == 0 && iteration == 0 {
                record_policy_approval(&host, &intent);
            }
            let admission = host
                .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
                .expect("accelerated budget admission");
            let PolicyDispatchAdmission::Granted { context } = admission else {
                panic!("expected accelerated budget admission")
            };
            let admission = context.admission();
            assert_eq!(admission.budget.task_daily_used, iteration as u32 + 1);
            assert_eq!(admission.budget.task_window_used, iteration as u32 + 1);
            assert_eq!(
                admission.budget.task_runtime_reserved_ms,
                iteration * 75_000 + 60_000
            );
            clock.advance(75_000);
            host.record_policy_dispatch_outcome(
                &intent.decision_id,
                &PolicyExecutionInput::Succeeded,
            )
            .expect("accelerated bounded execution");
            host.close().expect("close accelerated budget iteration");
        }

        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("accelerated exhausted budget runtime");
        let final_hour = day * 24 + 23;
        let unix_ms = start_unix_ms + final_hour * HOUR_MS;
        let evaluation = host
            .evaluate_policy_cycle_with_test_inputs(
                &policy_facts(),
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                209 + day * 10,
                PolicyTrigger::Reconciliation,
            )
            .expect("daily/window availability")
            .evaluation
            .unwrap();
        assert!(
            evaluation.dispatch_intents.is_empty(),
            "fifth daily/window execution cannot win"
        );
        assert!(
            evaluation.decisions[0]
                .reasons
                .iter()
                .any(|reason| reason.code == "policy_budget_exhausted")
        );
        host.close()
            .expect("close accelerated exhausted budget runtime");
    }
}
