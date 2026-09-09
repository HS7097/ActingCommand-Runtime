// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn policy_failure_activity_and_planning_facts_recover_without_duplicate_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
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
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected granted policy admission")
    };
    let budget_record = context.admission();
    assert_eq!(budget_record.budget.task_daily_used, 1);
    assert_eq!(budget_record.budget.activity_window_used, 1);
    assert!(budget_record.activity.seed > 0);

    let signals = [
        (
            "signal:goal-missed-a",
            PolicyPlanningSignalKind::GoalMissed,
            "goal.primary.missed",
        ),
        (
            "signal:feasibility-red-a",
            PolicyPlanningSignalKind::FeasibilityRed,
            "goal.primary.feasibility_red",
        ),
        (
            "signal:drift-predicted-a",
            PolicyPlanningSignalKind::DriftPredicted,
            "goal.primary.drift_predicted",
        ),
    ]
    .map(
        |(signal_id, kind, fact_code)| PolicyPlanningSignalEventData {
            signal_id: signal_id.to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: Some(intent.task_id.clone()),
            kind,
            fact_code: fact_code.to_owned(),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS + 50,
            detection_budget: None,
        },
    );
    for signal in &signals {
        host.record_policy_planning_signal(signal.clone())
            .expect("planning signal");
    }
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_some(),
        "informational planning facts must not pause or complete execution"
    );

    let failure_input = PolicyExecutionInput::Failed {
        error_code: "transient.capture".to_owned(),
        class: PolicyFailureClass::Recoverable,
    };
    clock.advance(100);
    let outcome = host
        .record_policy_dispatch_outcome(&intent.decision_id, &failure_input)
        .expect("policy failure outcome");
    let PolicyExecutionOutcome::Failed { failure } = &outcome.outcome else {
        panic!("expected classified failure")
    };
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(failure.escalation_streak, 1);
    assert!(!failure.performance_tax_exempt);
    assert_eq!(
        failure.perf_context.health,
        PerformanceMonitorHealth::Unavailable
    );
    assert_eq!(
        failure.perf_context.window_end_unix_ms,
        POLICY_NOW_UNIX_MS + 100
    );
    assert_eq!(
        failure.perf_context.window_start_unix_ms,
        POLICY_NOW_UNIX_MS + 100 - 30_000
    );
    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyPlanningSignalObserved)
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        match &event.payload {
            ProjectionPayload::Full(payload) => matches!(
                payload.as_ref(),
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.admission() == Some(budget_record)
            ),
            _ => false,
        }
    }));
    drop(client);
    host.close().expect("close host");

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let replay = reopened
        .record_policy_dispatch_outcome(&intent.decision_id, &failure_input)
        .expect("replay recovered policy outcome");
    assert_eq!(replay, outcome);
    for signal in signals {
        reopened
            .record_policy_planning_signal(signal)
            .expect("replay recovered planning signal");
    }
    let mut client = TestClient::connect(&reopened);
    let recovered = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_type == EventType::PolicyPlanningSignalObserved)
            .count(),
        3
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    drop(client);
    reopened.close().expect("close reopened host");
}
