// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn detection_planning_closed_activity_window_remains_nonfatal() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    host.activate_policy_catalog(&detection_policy_sources(1))
        .expect("activate detection catalog");

    for (now_unix_ms, window_open, snapshot_id) in [
        (
            POLICY_NOW_UNIX_MS - 5 * 3_600_000,
            false,
            "snapshot:detection-closed",
        ),
        (POLICY_NOW_UNIX_MS, true, "snapshot:detection-open"),
    ] {
        let mut facts = detection_policy_facts(false, snapshot_id);
        for fact in &mut facts.facts {
            fact.observed_at_unix_ms = now_unix_ms;
            fact.expires_at_unix_ms = Some(now_unix_ms + 60_000);
        }
        for outcome in &mut facts.outcomes {
            outcome.observed_at_unix_ms = now_unix_ms;
        }
        let mut resources = policy_resources();
        for pool in &mut resources.pools {
            pool.observed_at_unix_ms = now_unix_ms;
        }
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &resources,
                EvaluationTime {
                    unix_ms: now_unix_ms,
                    monotonic_ms: now_unix_ms,
                },
                1,
                PolicyTrigger::FactsChanged,
            )
            .expect("activity eligibility must not terminate detection evaluation");
        assert!(cycle.pending_dispatch_intents.is_empty());
        assert!(
            cycle
                .evaluation
                .as_ref()
                .expect("full detection evaluation")
                .decisions
                .iter()
                .any(|decision| {
                    decision.task_id == "fixture.detect"
                        && decision
                            .detection_suggestions
                            .iter()
                            .any(|suggestion| suggestion.fact_key == "detection.required")
                })
        );
        if window_open {
            assert_eq!(cycle.detection_planning_signals.len(), 1);
            let signal = &cycle.detection_planning_signals[0];
            assert_eq!(signal.kind, PolicyPlanningSignalKind::DetectionReserved);
            let budget = signal.detection_budget.as_ref().expect("detection budget");
            assert_eq!(budget.dispatch_used, 1);
            assert_eq!(budget.runtime_reserved_ms, 10_000);
        } else {
            assert!(cycle.detection_planning_signals.is_empty());
        }
        assert!(host.fatal_error().expect("runtime fatal state").is_none());
    }
    host.close().expect("close detection runtime");
}

#[test]
fn detection_quota_is_persistent_informational_and_never_starves_ordinary_work() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    host.activate_policy_catalog(&detection_policy_sources(1))
        .expect("activate detection catalog");

    let ordinary = host
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(true, "snapshot:detection-ordinary"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            1,
            PolicyTrigger::FactsChanged,
        )
        .expect("ordinary-first detection cycle");
    assert_eq!(ordinary.pending_dispatch_intents.len(), 1);
    assert_eq!(
        ordinary.pending_dispatch_intents[0].task_id,
        "fixture.observe"
    );
    assert!(ordinary.detection_planning_signals.is_empty());

    for index in 1_u64..=2 {
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &detection_policy_facts(false, &format!("snapshot:detection-reserved-{index}")),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + index * 2_000,
                    monotonic_ms: POLICY_NOW_UNIX_MS + index * 2_000,
                },
                index + 1,
                PolicyTrigger::FactsChanged,
            )
            .expect("reserve detection quota");
        assert!(cycle.pending_dispatch_intents.is_empty());
        assert_eq!(cycle.detection_planning_signals.len(), 1);
        let signal = &cycle.detection_planning_signals[0];
        assert_eq!(signal.kind, PolicyPlanningSignalKind::DetectionReserved);
        let budget = signal.detection_budget.as_ref().expect("detection budget");
        assert_eq!(
            budget.dispatch_used,
            u32::try_from(index).expect("small detection index")
        );
        assert_eq!(budget.runtime_reserved_ms, index * 10_000);
        if index == 1 {
            let mut forged = signal.clone();
            forged.signal_id = "signal:detection:caller-forged".to_owned();
            let error = host
                .record_policy_planning_signal(forged)
                .expect_err("callers cannot forge runtime-owned detection quota events");
            assert_eq!(error.code(), "policy_detection_signal_runtime_owned");
            assert!(!error.is_fatal());
        }
    }

    host.activate_policy_catalog(&detection_policy_sources(2))
        .expect("upgrade detection catalog without resetting quota");

    let exhausted = host
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(false, "snapshot:detection-exhausted"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 6_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 6_000,
            },
            4,
            PolicyTrigger::FactsChanged,
        )
        .expect("exhaust detection quota");
    assert!(exhausted.pending_dispatch_intents.is_empty());
    assert_eq!(exhausted.detection_planning_signals.len(), 1);
    assert_eq!(
        exhausted.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );

    let ordinary_after_exhaustion = host
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(true, "snapshot:detection-ordinary-after-exhaustion"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 7_000,
            },
            5,
            PolicyTrigger::FactsChanged,
        )
        .expect("ordinary work after detection quota exhaustion");
    assert_eq!(ordinary_after_exhaustion.pending_dispatch_intents.len(), 1);
    assert_eq!(
        ordinary_after_exhaustion.pending_dispatch_intents[0].task_id,
        "fixture.observe"
    );
    assert!(
        ordinary_after_exhaustion
            .detection_planning_signals
            .is_empty()
    );

    let mut client = TestClient::connect(&host);
    let signals = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signals.len(), 3);
    assert!(
        signals
            .iter()
            .all(|event| event.severity == EventSeverity::Info)
    );
    assert_eq!(
        signals
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if matches!(
                            payload.as_ref(),
                            EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal))
                                if signal.kind()
                                    == PolicyPlanningSignalKind::DetectionQuotaExhausted
                        )
                )
            })
            .count(),
        1
    );
    drop(client);
    let mut dispatch_client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut dispatch_client,
            EventQuery {
                event_type: Some(EventType::PolicyDispatchIntent),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "detection reservations must not be recorded as ordinary dispatch success"
    );
    drop(dispatch_client);
    host.close().expect("close detection runtime");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen detection runtime");
    let recovered = reopened
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(false, "snapshot:detection-after-restart"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 9_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 9_000,
            },
            6,
            PolicyTrigger::Recovery,
        )
        .expect("recover exhausted detection quota");
    assert_eq!(recovered.detection_planning_signals.len(), 1);
    assert_eq!(
        recovered.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    assert!(recovered.pending_dispatch_intents.is_empty());
    reopened.close().expect("close recovered detection runtime");
}

#[test]
fn tightened_detection_quota_preserves_historical_usage_and_recovers() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    host.activate_policy_catalog(&detection_policy_sources_with_budget(1, 3, 30_000, 10_000))
        .expect("activate initial detection catalog");

    for index in 1_u64..=2 {
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &detection_policy_facts(false, &format!("snapshot:tighten-{index}")),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + index * 1_000,
                    monotonic_ms: POLICY_NOW_UNIX_MS + index * 1_000,
                },
                index,
                PolicyTrigger::FactsChanged,
            )
            .expect("reserve initial detection quota");
        assert_eq!(
            cycle.detection_planning_signals[0].kind,
            PolicyPlanningSignalKind::DetectionReserved
        );
    }

    host.activate_policy_catalog(&detection_policy_sources_with_budget(2, 1, 5_000, 5_000))
        .expect("tighten detection catalog");
    let exhausted = host
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(false, "snapshot:tightened-exhausted"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 3_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 3_000,
            },
            3,
            PolicyTrigger::CatalogChanged,
        )
        .expect("tightened quota is informational");
    assert_eq!(exhausted.detection_planning_signals.len(), 1);
    let signal = &exhausted.detection_planning_signals[0];
    assert_eq!(
        signal.kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    let budget = signal.detection_budget.as_ref().expect("detection budget");
    assert_eq!(budget.dispatch_used, 2);
    assert_eq!(budget.dispatch_limit, 1);
    assert_eq!(budget.runtime_reserved_ms, 20_000);
    assert_eq!(budget.runtime_limit_ms, 5_000);
    host.close().expect("close tightened detection runtime");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen tightened detection runtime");
    let recovered = reopened
        .evaluate_policy_cycle_with_test_inputs(
            &detection_policy_facts(false, "snapshot:tightened-recovered"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 4_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 4_000,
            },
            4,
            PolicyTrigger::Recovery,
        )
        .expect("recover historical over-limit quota");
    assert_eq!(recovered.detection_planning_signals.len(), 1);
    assert_eq!(
        recovered.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    reopened.close().expect("close recovered detection runtime");
}
