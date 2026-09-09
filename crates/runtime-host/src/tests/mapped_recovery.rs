// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn mapped_recovery_fails_loud_for_missing_terminal_and_partial_links() {
    for case in ["missing-terminal", "missing-disposition", "partial-links"] {
        let outcome_key = format!("recovery-{case}");
        let root = TempDir::new().expect("tempdir");
        let package =
            neutral_mapped_contained_task_package(&outcome_key, "designated_effect_completed");
        let (host, state, context, _request) = admitted_mapped_run_fixture(&root, &outcome_key);
        let expected_code = match case {
            "missing-terminal" => {
                host.complete_scheduled_policy_success_without_terminal_for_test(&context)
                    .expect("persist full scheduled completion without terminal");
                "policy_run_terminal_missing"
            }
            "missing-disposition" => {
                host.append_contained_task_terminal_for_test(
                    context.request(),
                    context.lease_token(),
                    context.issued_task_id(),
                    context.issued_run_id(),
                    TaskOutcome::Success,
                    false,
                    Some("neutral/terminal".to_owned()),
                    0,
                    None,
                )
                .expect("persist success terminal without mapped disposition");
                host.complete_scheduled_policy_success_without_terminal_for_test(&context)
                    .expect("persist scheduled completion after generic terminal");
                "policy_run_terminal_disposition_missing"
            }
            "partial-links" => {
                let error = host
                    .complete_policy_success_with_partial_links_for_test(&context)
                    .expect_err("mapped completion with partial links must fail");
                assert_eq!(error.code(), "policy_run_identity_missing");
                "policy_run_identity_missing"
            }
            _ => unreachable!(),
        };
        let close = host.close();
        assert!(
            close.is_ok()
                || close
                    .as_ref()
                    .is_err_and(|error| error.code() == expected_code),
            "{case}: unexpected close result {close:?}"
        );

        let restarted = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(
                    mapped_policy_facts(&outcome_key, false),
                    policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(
                FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    context.lease_token().instance_id(),
                    state,
                )
                .fixture_simulation(),
            ),
        );
        let error = match restarted {
            Ok(host) => {
                host.close().expect("close unexpectedly restarted host");
                panic!("{case}: invalid mapped recovery evidence must fail startup")
            }
            Err(error) => error,
        };
        assert_eq!(error.code(), expected_code, "{case}");
    }
}

#[test]
fn mapped_failed_completion_without_a_terminal_recovers_without_an_outcome() {
    let outcome_key = "recovery-failed-no-terminal";
    let root = TempDir::new().expect("tempdir");
    let package = neutral_mapped_contained_task_package(outcome_key, "designated_effect_completed");
    let (host, state, context, _request) = admitted_mapped_run_fixture(&root, outcome_key);
    let completion = host
        .complete_scheduled_policy_failure_without_terminal_for_test(&context)
        .expect("persist failed scheduled completion without terminal");
    assert!(matches!(
        completion.outcome,
        PolicyExecutionOutcome::Failed { .. }
    ));
    host.close().expect("close failed scheduled host");

    let restarted = RuntimeHost::start(
        config(&root)
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
                context.lease_token().instance_id(),
                state,
            )
            .fixture_simulation(),
        ),
    )
    .expect("failed completion without terminal is a valid recovery record");
    let cycle = restarted
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts(outcome_key, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 120_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 120_000,
            },
            12_003,
            PolicyTrigger::Recovery,
        )
        .expect("evaluate recovered failed completion");
    assert!(
        cycle
            .evaluation
            .expect("recovered failed evaluation")
            .dispatch_intents
            .iter()
            .all(|intent| intent.task_id != "fixture.followup")
    );
    restarted
        .close()
        .expect("close recovered failed scheduled host");
}

#[test]
fn mapped_completion_request_identity_is_bound_to_the_admission_event() {
    let root = TempDir::new().expect("tempdir");
    let (host, _state, context, request) =
        admitted_mapped_run_fixture(&root, "mapped-request-identity");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("mapped request-identity run");
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("mapped request-identity completion");
    let terminal_sequence = receipt.terminal().expect("mapped terminal").sequence;
    host.validate_policy_admission_request_for_test(
        &context,
        context.admission_request_id(),
        terminal_sequence,
    )
    .expect("exact admission request");
    let wrong_request_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_request_id()
        .expect("wrong request id")
        .transport();
    let error = host
        .validate_policy_admission_request_for_test(&context, wrong_request_id, terminal_sequence)
        .expect_err("wrong admission request must fail closed");
    assert_eq!(error.code(), "policy_run_admission_conflict");
    host.close().expect("close request-identity host");
}

#[test]
fn authoritative_outcome_state_advances_only_from_ordered_exact_run_projections() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let instance_id = ids.mint_instance_id().expect("instance id");
    let task_id = ids.mint_task_id().expect("task id");
    let lease_id = ids.mint_lease_id().expect("lease id");
    let make_outcome = |sequence: u64, suffix: &str| {
        AuthoritativeSchedulingOutcome::new(
            SchedulingOutcomeIdentity::new(
                *ids.mint_event_id().expect("terminal event id").transport(),
                sequence,
                *instance_id.transport(),
                *task_id.transport(),
                *ids.mint_run_id().expect("run id").transport(),
                *ids.mint_request_id().expect("request id").transport(),
                *ids.mint_correlation_id()
                    .expect("correlation id")
                    .transport(),
                *lease_id.transport(),
                format!("decision-{suffix}"),
                "fixture.observe",
                POLICY_INSTANCE_ALIAS,
            )
            .expect("exact outcome identity"),
            SchedulingDisposition::new(
                format!("outcome-{suffix}"),
                SchedulingEffectEvidence::NoDesignatedEffect,
            )
            .expect("outcome disposition"),
            POLICY_NOW_UNIX_MS + sequence,
        )
        .expect("authoritative outcome")
    };
    let older = make_outcome(10, "older");
    let newer = make_outcome(20, "newer");
    let mut outcomes = BTreeMap::new();

    insert_authoritative_policy_outcome(&mut outcomes, older.clone()).expect("first exact outcome");
    insert_authoritative_policy_outcome(&mut outcomes, newer.clone())
        .expect("ordered exact outcome");
    insert_authoritative_policy_outcome(&mut outcomes, newer.clone())
        .expect("identical exact replay");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes.values().next(), Some(&newer));
    let error = insert_authoritative_policy_outcome(&mut outcomes, older)
        .expect_err("out-of-order exact replay must fail closed");
    assert_eq!(error.code(), "policy_outcome_replay_order_conflict");
}

#[test]
fn mapped_terminal_append_failure_returns_no_success_or_disposition() {
    let root = TempDir::new().expect("tempdir");
    let (host, state, context, request) = admitted_mapped_run_fixture(&root, "append-failure");
    host.fail_next_scheduling_terminal_append_for_test()
        .expect("inject terminal append failure");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("terminal append failure cannot return a success receipt");
    assert_eq!(error.code(), "scheduling_terminal_append_injected_failure");
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let events = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        })
        .expect("query mapped append failure events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::TaskCompleted)
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload(),
                EventPayload::Task(TaskPayload::Semantic(payload))
                    if matches!(
                        payload.fact(),
                        TaskSemanticFact::TerminalCommitted {
                            scheduling_disposition: Some(_),
                            ..
                        }
                    )
            ))
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type() == EventType::PolicyDispatchAdmitted)
            .count(),
        1,
        "append failure cannot admit a downstream task"
    );
    let close = host.close();
    assert!(
        close.is_ok()
            || close
                .as_ref()
                .is_err_and(|error| error.code() == "scheduling_terminal_append_injected_failure")
    );
}
