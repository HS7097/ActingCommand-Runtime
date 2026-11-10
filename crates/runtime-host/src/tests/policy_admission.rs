// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn policy_cadence_is_explicit_and_clock_jumps_force_full_recompute() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let facts = policy_facts();
    let resources = policy_resources();

    let startup = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("startup policy cycle");
    assert_eq!(startup.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        startup.directive.reason,
        PolicyRecomputeReason::StartupOrRecovery
    );
    assert!(startup.evaluation.is_some());
    let startup_measurement = startup.measurement.expect("startup measurement");
    assert_eq!(
        startup_measurement.requested_recompute,
        PolicyRecomputeKind::Full
    );
    assert_eq!(
        startup_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    assert!(startup_measurement.cost.work_units > 0);

    let cooldown = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 100,
            },
            7,
            PolicyTrigger::ResourcesChanged,
        )
        .expect("cooldown policy cycle");
    assert_eq!(cooldown.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(cooldown.directive.reason, PolicyRecomputeReason::Cooldown);
    assert!(cooldown.evaluation.is_none());
    assert!(cooldown.measurement.is_none());

    let incremental = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 1_100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("incremental policy cycle");
    assert_eq!(incremental.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(incremental.directive.reason, PolicyRecomputeReason::Event);
    let incremental_measurement = incremental.measurement.expect("incremental measurement");
    assert_eq!(
        incremental_measurement.requested_recompute,
        PolicyRecomputeKind::Incremental
    );
    assert_eq!(
        incremental_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    // Workflow #308 slice 5b: the pair found eligible at startup now carries its eligibility
    // age, one more task-state record; every other input dimension is unchanged.
    assert_eq!(
        incremental_measurement.cost,
        PolicyEvaluationCost {
            task_state_records: startup_measurement.cost.task_state_records + 1,
            work_units: startup_measurement.cost.work_units + 1,
            ..startup_measurement.cost
        }
    );
    assert!(
        incremental_measurement.sampled_at_monotonic_ms
            >= startup_measurement.sampled_at_monotonic_ms
    );

    let clock_jump = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 7_000,
            },
            7,
            PolicyTrigger::ClockObserved {
                previous_unix_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
        )
        .expect("clock-jump policy cycle");
    assert_eq!(clock_jump.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        clock_jump.directive.reason,
        PolicyRecomputeReason::ClockJump
    );

    let reconciliation = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 67_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 67_000,
            },
            7,
            PolicyTrigger::Reconciliation,
        )
        .expect("reconciliation policy cycle");
    assert_eq!(reconciliation.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        reconciliation.directive.reason,
        PolicyRecomputeReason::Reconciliation
    );
    host.close().expect("close host");
}

#[test]
fn catalog_cas_conflict_preserves_nonfatal_identity_and_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let first = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let second = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");

    let (result, document) = host
        .activate_policy_catalog_with_expected_for_test(&policy_sources(3), first)
        .expect("invoke catalog compare-and-swap and read committed state");
    let error = result.expect_err("stale compare-and-swap must fail");
    assert_eq!(error.code(), "catalog_active_generation_changed");
    assert_eq!(error.operation(), "switch_active_catalog");
    assert!(!error.is_fatal());
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("active generation"),
        second
    );

    let document = document.expect("active pointer");
    let pointer: serde_json::Value =
        serde_json::from_slice(document.payload()).expect("catalog pointer");
    assert_eq!(
        pointer["generation"]["catalog_hash"],
        second.catalog_hash(),
        "confirmed rollback preserves the durable pointer"
    );
    let mut client = TestClient::connect(&host);
    let failures = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogTransitionFailed),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) =
        &failures.last().expect("catalog transition failure").payload
    else {
        panic!("expected full catalog transition failure payload");
    };
    assert_eq!(
        payload.effect_disposition(),
        Some(EffectDisposition::NotPerformed)
    );

    host.activate_policy_catalog(&policy_sources(3))
        .expect("runtime remains usable after stale compare-and-swap");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let first_catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let forged_approval = policy_context(&host, &intent);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &forged_approval)
        .expect_err("caller-supplied approval IDs must not grant authority");
    assert_eq!(error.code(), "policy_approval_fact_missing");
    record_policy_approval(&host, &intent);

    let mut tampered_intent = intent.clone();
    tampered_intent.decision_id = "decision:tampered".to_owned();
    tampered_intent.reason_chain_id = "reason:tampered".to_owned();
    tampered_intent.approval_refs.clear();
    let mut tampered_reasons = reasons.clone();
    tampered_reasons.id = "reason:tampered".to_owned();
    tampered_reasons.decision_id = "decision:tampered".to_owned();
    let error = host
        .admit_policy_dispatch(
            &tampered_intent,
            &tampered_reasons,
            &policy_context(&host, &tampered_intent),
        )
        .expect_err("catalog approval requirements cannot be stripped");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    let (_, approved_intent, approved_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        8,
    );

    let admission = host
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&host, &approved_intent),
        )
        .expect("policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    let mut stale_context = policy_context(&host, &approved_intent);
    stale_context.now_unix_ms = POLICY_NOW_UNIX_MS + 60_000;
    let replay = host
        .admit_policy_dispatch(&approved_intent, &approved_reasons, &stale_context)
        .expect("exact replay is suppressed before mutable-state revalidation");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let after = projected_events(&mut client, EventQuery::default());
    assert_eq!(before.len(), after.len());
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
            .count(),
        2
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
            .count(),
        1
    );

    let second_catalog = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    assert_ne!(first_catalog.catalog_hash(), second_catalog.catalog_hash());
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut old_new_intent = intent.clone();
    old_new_intent.decision_id = "decision:fixture-b".to_owned();
    old_new_intent.reason_chain_id = "reason:fixture-b".to_owned();
    let mut old_new_reasons = reasons.clone();
    old_new_reasons.id = "reason:fixture-b".to_owned();
    old_new_reasons.decision_id = "decision:fixture-b".to_owned();
    let error = host
        .admit_policy_dispatch(
            &old_new_intent,
            &old_new_reasons,
            &policy_context(&host, &old_new_intent),
        )
        .expect_err("new admission cannot use the old catalog");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    host.complete_policy_dispatch(&approved_intent.decision_id)
        .expect("complete policy dispatch");
    assert!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .is_none()
    );
    let rolled_back = host
        .rollback_policy_catalog(first_catalog.catalog_hash())
        .expect("rollback policy catalog");
    assert_eq!(rolled_back, first_catalog);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogActivated)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogRolledBack)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchCompleted)
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );
    let replay = reopened
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&reopened, &approved_intent),
        )
        .expect("replay after restart");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close reopened host");

    // SCHEDULING-INSTANCE-ALIAS-v1; first red: Workflow #269 comment 5578055809.
    let alias = " Instance Ω ";
    let root = TempDir::new().expect("alias state");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let mut facts = policy_facts();
    facts.instances[0].instance_id = alias.to_owned();
    facts.outcomes[0].instance_id = alias.to_owned();
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources())),
        Arc::new(FakeProvider::one(alias, registered_id, Arc::clone(&state))),
    )
    .expect("alias host");
    let mut sources = policy_sources(1);
    let encoded = serde_json::to_string(alias).expect("alias JSON");
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        source.bytes = String::from_utf8(source.bytes.clone())
            .expect("source UTF-8")
            .replace("\"fixture-instance-a\"", &encoded)
            .into_bytes();
    }
    let mut tasks: serde_json::Value = serde_json::from_slice(&sources.tasks.bytes).unwrap();
    tasks["tasks"][0]["trigger"] = serde_json::json!({"kind":"fact","scope":{"kind":"instance","instance_id":alias},"fact_key":"env.alias_ready","comparison":"eq","value":{"type":"boolean","value":true},"max_age_ms":60000});
    sources.tasks.bytes = serde_json::to_vec(&tasks).unwrap();
    host.activate_policy_catalog(&sources)
        .expect("compile exact alias");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: alias.to_owned(),
        },
        "env.alias_ready",
        ContractFactValue::Boolean(true),
        "snapshot:alias-ready",
        Vec::new(),
    ))
    .expect("publish exact alias fact");
    let context = InstanceFactContext {
        instance_id: alias.to_owned(),
        server_id: "fixture-server-a".to_owned(),
        game_id: "fixture-game-a".to_owned(),
    };
    assert_eq!(
        host.instance_fact_snapshot(context.clone())
            .expect("exact alias fact snapshot")
            .context
            .instance_id,
        alias
    );
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("alias evaluation");
    let evaluation = cycle.evaluation.expect("evaluation");
    let [intent] = evaluation.dispatch_intents.as_slice() else {
        panic!("one alias intent: {evaluation:?}")
    };
    assert_eq!(intent.instance_id, alias);
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|reason| reason.id == intent.reason_chain_id)
        .expect("reason chain");
    record_policy_approval(&host, intent);
    assert!(matches!(
        host.admit_policy_dispatch(intent, reasons, &policy_context(&host, intent))
            .expect("exact alias admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    assert!(matches!(
        host.admit_policy_dispatch(intent, reasons, &policy_context(&host, intent))
            .expect("exact alias replay"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let mut client = TestClient::connect(&host);
    for unknown in [" instance Ω ", "unknown.instance"] {
        let request = client.request(RuntimeOperation::ObserveReadonly {
            instance_alias: unknown.to_owned(),
        });
        let denied = client.send(&request);
        assert_eq!(denied.state(), RuntimeReceiptState::Denied);
        assert_eq!(
            denied.error_projection().expect("unknown alias").code,
            RuntimeErrorCode::InstanceUnknown
        );
    }
    let events = projected_events(&mut client, EventQuery::default());
    let admitted = events
        .iter()
        .find(|event| event.event_type == EventType::PolicyDispatchAdmitted)
        .expect("native alias admission");
    assert_eq!(admitted.links.instance_id(), Some(&registered_id));
    let ProjectionPayload::Full(payload) = &admitted.payload else {
        panic!("full policy payload")
    };
    let EventPayload::Policy(actingcommand_contract::PolicyPayload::DispatchAdmitted(data)) =
        payload.as_ref()
    else {
        panic!("dispatch admission")
    };
    assert_eq!(data.instance_id(), alias);
    host.complete_policy_dispatch(&intent.decision_id)
        .expect("complete alias dispatch");
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close alias host");
    let reopened = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources())),
        Arc::new(FakeProvider::one(alias, registered_id, state)),
    )
    .expect("reopen exact alias");
    assert_eq!(
        reopened
            .instance_fact_snapshot(context)
            .expect("recovered alias fact")
            .context
            .instance_id,
        alias
    );
    assert!(matches!(
        reopened
            .admit_policy_dispatch(intent, reasons, &policy_context(&reopened, intent))
            .expect("recovered replay"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close alias replay host");
}

#[test]
fn capacity_pressure_refuses_policy_admission_before_lease_and_recovers_in_place() {
    // Specification criterion: Workflow #287, issuecomment-5664237218.
    use actingcommand_contract::{
        CapacityAdmissionOutcome, CapacityAdmissionReason, CapacityPurpose, CapacityState,
        CapacityThresholds, PerformancePayload,
    };
    use actingcommand_host_metrics::{
        CapacitySample, CapacityTarget, HostSample, HostSampler, ProcessLoadThresholds,
    };

    struct AdmissionCapacitySampler {
        available_bytes: Arc<AtomicU64>,
    }

    impl HostSampler for AdmissionCapacitySampler {
        fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
            let available_bytes = self.available_bytes.load(Ordering::Acquire);
            actingcommand_host_metrics::sample_capacity(targets)
                .into_iter()
                .map(|mut sample| {
                    if sample.available_bytes.is_ok() {
                        sample.available_bytes = Ok(available_bytes);
                    }
                    sample
                })
                .collect()
        }

        fn sample(
            &mut self,
            _observed_at_unix_ms: u64,
            _owned_processes: &BTreeMap<u32, String>,
            _top_process_count: usize,
            _thresholds: ProcessLoadThresholds,
        ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
            panic!("capacity-only specification does not enable performance counters")
        }
    }

    for scheduled in [false, true] {
        let root = TempDir::new().expect("capacity admission tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let state = Arc::new(FakeState::default());
        let thresholds = CapacityThresholds::default();
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_capacity_thresholds(thresholds),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                instance_id(),
                Arc::clone(&state),
            )),
        )
        .expect("capacity admission host");
        host.activate_policy_catalog(&policy_sources(1))
            .expect("activate capacity admission catalog");
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let admission_context = policy_context(&host, &intent);
        // Workflow #336 PKG-S3: a dispatch names no package digest; the request's own
        // reference is compared with nothing (PKG-S4 removes it).
        let task_request = ContainedTaskRequest::new(
            root.path()
                .join("capacity-admission.zip")
                .to_string_lossy()
                .into_owned(),
        )
        .expect("declared scheduled request");
        let available_bytes = Arc::new(AtomicU64::new(0));
        host.replace_capacity_sampler_for_test(Box::new(AdmissionCapacitySampler {
            available_bytes: Arc::clone(&available_bytes),
        }))
        .expect("commit hard-pressure capacity through the original owner");

        let failure = if scheduled {
            host.admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &admission_context,
                &task_request,
            )
        } else {
            host.admit_policy_dispatch(&intent, &reasons, &admission_context)
        }
        .expect_err("hard pressure must refuse before lease request");
        assert_eq!(failure.code(), "capacity_admission_refused");
        assert!(!failure.is_fatal());
        let decision = failure
            .lifecycle
            .capacity
            .as_ref()
            .expect("capacity decision");
        decision.validate().expect("valid refusal decision");
        assert_eq!(
            decision.admission_result,
            CapacityAdmissionOutcome::HardPressure
        );
        assert_eq!(decision.reason, CapacityAdmissionReason::HardThreshold);
        assert_eq!(decision.owner_epoch, host.runtime_info().owner_epoch());
        assert_eq!(decision.requested_bytes, 0);
        assert!(decision.target_volume.is_none());
        let reference = decision
            .fact
            .as_ref()
            .expect("committed capacity reference");
        let samples = host
            .query_persisted_events_for_test(EventQuery {
                from_sequence: Some(reference.sequence),
                to_sequence: Some(reference.sequence),
                event_type: Some(EventType::PerformanceSummary),
                ..EventQuery::default()
            })
            .expect("read the decision's actual committed sample");
        assert_eq!(samples.len(), 1);
        let sample_event = &samples[0];
        assert_eq!(sample_event.event_id(), &reference.event_id);
        assert_eq!(sample_event.sequence(), reference.sequence);
        let EventPayload::Performance(PerformancePayload::Summary(payload)) =
            sample_event.payload()
        else {
            panic!("capacity summary payload")
        };
        let sample = payload.capacity().expect("capacity sample");
        sample.validate().expect("valid committed sample");
        assert_eq!(sample.owner_epoch, decision.owner_epoch);
        assert_eq!(reference.owner_epoch, sample.owner_epoch);
        assert_eq!(sample.thresholds, thresholds);
        assert_eq!(reference.observed_at_unix_ms, sample.observed_at_unix_ms);
        assert_eq!(
            reference.observed_at_monotonic_ms,
            sample.observed_at_monotonic_ms
        );
        assert_eq!(decision.decided_at_unix_ms, POLICY_NOW_UNIX_MS);
        assert_eq!(decision.decided_at_monotonic_ms, 0);
        assert!(
            decision
                .decided_at_unix_ms
                .checked_sub(sample.observed_at_unix_ms)
                .is_some_and(|age| age <= sample.freshness_ms)
        );
        assert!(
            decision
                .decided_at_monotonic_ms
                .checked_sub(sample.observed_at_monotonic_ms)
                .is_some_and(|age| age <= sample.freshness_ms)
        );
        let state_volume = actingcommand_host_metrics::capacity_volume(root.path())
            .expect("actual temporary state volume");
        assert!(sample.volumes.iter().any(|volume| {
            volume.purposes.contains(&CapacityPurpose::State)
                && volume.volume_id.as_ref() == Some(&state_volume)
        }));
        assert!(sample.volumes.iter().all(|volume| {
            volume.available_bytes == Some(0)
                && volume.state == CapacityState::HardPressure
                && volume.cause.is_none()
        }));

        let rejected = host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchRejected),
                ..EventQuery::default()
            })
            .expect("capacity refusal fact");
        assert_eq!(rejected.len(), 1);
        let rejected = &rejected[0];
        let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = rejected.payload()
        else {
            panic!("policy refusal payload")
        };
        assert_eq!(payload.decision_id(), intent.decision_id);
        assert_eq!(payload.rejection(), Some(&failure.policy_rejection()));
        assert_eq!(
            rejected.payload().effect_disposition(),
            Some(EffectDisposition::NotPerformed)
        );
        let rejected_request_id = *rejected
            .links()
            .request_id()
            .expect("refused request identity");
        let request_events = host
            .query_persisted_events_for_test(EventQuery {
                request_id: Some(rejected_request_id),
                ..EventQuery::default()
            })
            .expect("all facts for the refused request");
        for forbidden in [
            EventType::PolicyDispatchAdmitted,
            EventType::LeaseRequested,
            EventType::LeaseGranted,
        ] {
            assert!(
                request_events
                    .iter()
                    .all(|event| event.event_type() != forbidden),
                "scheduled={scheduled}: capacity refusal must precede {forbidden:?}"
            );
        }
        assert_eq!(state.open_count.load(Ordering::Acquire), 0);
        assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);

        available_bytes.store(thresholds.soft_bytes, Ordering::Release);
        clock.advance(60_000);
        host.capacity_sampler_for_test()
            .expect("existing capacity sampling entry")()
        .expect("commit sufficient capacity in the same host");
        let recovery_time = POLICY_NOW_UNIX_MS + 60_000;
        let recovery_samples = host
            .query_persisted_events_for_test(EventQuery {
                from_sequence: Some(reference.sequence),
                event_type: Some(EventType::PerformanceSummary),
                ..EventQuery::default()
            })
            .expect("read committed recovery samples");
        let recovered = recovery_samples
            .iter()
            .find(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Performance(PerformancePayload::Summary(payload))
                        if payload.capacity().is_some_and(|sample| {
                            sample.owner_epoch == decision.owner_epoch
                                && sample.observed_at_unix_ms == recovery_time
                                && sample.observed_at_monotonic_ms == 60_000
                                && sample.thresholds == thresholds
                                && sample.volumes.iter().all(|volume| {
                                    volume.available_bytes == Some(thresholds.soft_bytes)
                                        && volume.state == CapacityState::Sufficient
                                        && volume.cause.is_none()
                                })
                        })
                )
            })
            .expect("same owner committed a sufficient recovery fact");
        assert!(recovered.sequence() > reference.sequence);
        assert_ne!(recovered.event_id(), &reference.event_id);
        let (_, restored_intent, restored_reasons) =
            evaluated_policy_dispatch_at(&host, PolicyTrigger::Reconciliation, recovery_time, 8);
        assert_ne!(restored_intent.decision_id, intent.decision_id);
        let restored_context = policy_context(&host, &restored_intent);
        let restored_task_request = ContainedTaskRequest::new(task_request.package_path())
            .expect("declared recovery request");
        let admission = if scheduled {
            host.admit_scheduled_policy_dispatch(
                &restored_intent,
                &restored_reasons,
                &restored_context,
                &restored_task_request,
            )
        } else {
            host.admit_policy_dispatch(&restored_intent, &restored_reasons, &restored_context)
        }
        .expect("fresh sufficient fact must restore admission");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected a newly granted recovery admission")
        };
        assert_ne!(context.request().request_id(), rejected_request_id);
        let granted = host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchAdmitted),
                request_id: Some(context.request().request_id()),
                ..EventQuery::default()
            })
            .expect("recovery admission fact");
        assert_eq!(granted.len(), 1);
        assert!(granted[0].sequence() > recovered.sequence());
        assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
        assert_eq!(state.input_count.load(Ordering::Acquire), 0);
        host.close()
            .expect("original shutdown releases the granted lease and closes the host");
    }
}

#[test]
fn policy_final_admission_records_the_actual_control_rejection() {
    // Workflow #269 B11 first red: issuecomment-5587376490.
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state.clone(),
        )),
    )
    .expect("control rejection host");
    let mut sources = policy_sources(1);
    let mut activity: serde_json::Value = serde_json::from_slice(&sources.activity.bytes).unwrap();
    let end_minute = (POLICY_NOW_UNIX_MS % 86_400_000) / 60_000 + 1;
    activity["profiles"][0]["windows"][0]["start_minute_of_day"] = serde_json::json!(0);
    activity["profiles"][0]["windows"][0]["end_minute_of_day"] = serde_json::json!(end_minute);
    sources.activity.bytes = serde_json::to_vec(&activity).unwrap();
    host.activate_policy_catalog(&sources)
        .expect("activate closing window");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    clock.advance(60_000);
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("capacity sample at final admission time");
    let failure = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("selection cannot bypass final locked window admission");
    assert_eq!(failure.code(), "policy_activity_window_closed");
    assert!(!failure.is_fatal());
    let events = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchRejected),
            ..EventQuery::default()
        })
        .expect("actual rejected event");
    assert_eq!(events.len(), 1);
    let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = events[0].payload() else {
        panic!("rejection payload")
    };
    let rejection = payload.rejection().expect("original RequestFailure facts");
    assert_eq!(rejection, &failure.policy_rejection());
    // Ledger revision 3 (#378 A3): the refusal is the rejection's outcome, its operation a value.
    assert_eq!(
        outcome_str(&rejection.outcome, "operation"),
        Some("reserve_policy_budget")
    );
    assert_eq!(
        rejection.next_eligible_unix_ms,
        Some((POLICY_NOW_UNIX_MS / 86_400_000 + 1) * 86_400_000)
    );
    assert!(rejection.budget.is_none());
    assert_eq!(payload.reasons().len(), reasons.reasons.len());
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        host.query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchAdmitted),
            ..EventQuery::default()
        })
        .unwrap()
        .is_empty()
    );
    let original = events[0].clone();
    host.close().expect("close rejected host");
    let restarted = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state,
        )),
    )
    .expect("replay rejection without changing ranking identity");
    let replayed = restarted
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchRejected),
            ..EventQuery::default()
        })
        .unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].payload(), original.payload());
    assert_eq!(replayed[0].sequence(), original.sequence());
    restarted.close().expect("close replayed host");
}

/// Test plan v2.2 §4 #12 (HOST-R3): the fact ages past its freshness on the Runtime's manual
/// clock instead of a 30 ms sleep, and admission then refuses `policy_facts_stale`
/// (`contracts/scheduling/README.md:340-345`: a fact the evaluation read bounds
/// `facts_fresh_until_unix_ms`, and admission refuses an intent past it).
#[test]
fn policy_admission_rejects_stale_and_tampered_trusted_context() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 7_000));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("tasks fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "env.ephemeral",
        "comparison": "eq",
        "value": {"type": "string", "value": "Neutral"},
        "max_age_ms": 20
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog");
    let mut ephemeral = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ephemeral",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:ephemeral",
        Vec::new(),
    );
    ephemeral.expires_at_unix_ms = Some(POLICY_NOW_UNIX_MS + 20);
    ephemeral.ttl_policy = Some(FactTtlPolicy {
        minimum_ms: 1,
        maximum_ms: 100,
        source: FactTtlSource::DetectorContract,
    });
    host.publish_fact(ephemeral)
        .expect("publish ephemeral fact");

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut forged_decision = intent.clone();
    forged_decision.decision_id = "decision:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &forged_decision,
            &reasons,
            &policy_context(&host, &forged_decision),
        )
        .expect_err("forged decision identity must fail")
        .code(),
        "policy_decision_not_host_evaluated"
    );

    let mut wrong_profile = intent.clone();
    wrong_profile.prerequisites.activity_profile_id = "profile:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_profile,
            &reasons,
            &policy_context(&host, &wrong_profile),
        )
        .expect_err("caller-selected profile must fail")
        .code(),
        "policy_trusted_context_mismatch"
    );

    // The fact's 20 ms freshness ends while 30 ms pass on the Runtime's clock.
    clock.advance(30);
    let mut forged_time = policy_context(&host, &intent);
    forged_time.now_unix_ms = 1;
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &forged_time)
            .expect_err("expired fact must fail admission")
            .code(),
        "policy_facts_stale"
    );
    host.close().expect("close host");
}

#[test]
fn runtime_owned_policy_inputs_supply_time_and_reject_unknown_resource_hosts() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 7_000));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock)
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let cycle = host
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("Runtime-owned policy evaluation");
    let intent = cycle
        .evaluation
        .expect("policy evaluation")
        .dispatch_intents
        .into_iter()
        .next()
        .expect("dispatch intent");
    assert_eq!(
        intent.prerequisites.evaluated_at_unix_ms,
        POLICY_NOW_UNIX_MS
    );
    host.close().expect("close host");

    let invalid_root = TempDir::new().expect("tempdir");
    let mut invalid_facts = policy_facts();
    invalid_facts.instances[0].host_id = "caller-forged-host".to_owned();
    let invalid = RuntimeHost::start(
        config(&invalid_root)
            .with_policy_inputs(PolicyInputSnapshot::new(invalid_facts, policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host with invalid policy inputs");
    invalid
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    // ISO-PE (#419): the refusal is not fatal and not global, so the evaluation is skipped with it
    // (recorded as `policy_evaluation_skipped`) instead of returning it, and nothing dispatches.
    let cycle = invalid
        .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
        .expect("unknown resource host must fail: the evaluation is skipped");
    assert!(cycle.evaluation.is_none());
    assert_eq!(
        super::gate_419_iso_pe::skipped_failure_codes(&invalid),
        vec!["policy_resource_metadata_untrusted".to_owned()]
    );
    assert!(invalid.fatal_error().expect("health").is_none());
    invalid.close().expect("close invalid host");
}

#[test]
fn fact_replacement_invalidates_an_already_evaluated_dispatch() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    host.publish_fact(stored_fact(
        scope.clone(),
        "env.authority",
        ContractFactValue::Boolean(true),
        "snapshot:authority-a",
        Vec::new(),
    ))
    .expect("publish initial fact revision");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut replacement = stored_fact(
        scope,
        "env.authority",
        ContractFactValue::Boolean(true),
        "snapshot:authority-b",
        Vec::new(),
    );
    replacement.observed_at_unix_ms += 1;
    host.publish_fact(replacement)
        .expect("replace fact revision");

    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("superseded fact revision must invalidate dispatch");
    assert_eq!(error.code(), "policy_facts_stale");
    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn procedure_manifest_rejects_yield_point_mismatch_during_evaluation() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_procedure_manifest(procedure_manifest_with_primary(
            b"fixture procedure observe package v1",
            vec!["different_boundary".to_owned()],
        )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let error = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect_err("manifest yield points must match the catalog intent");
    assert_eq!(error.code(), "procedure_yield_points_mismatch");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn policy_evaluation_fails_explicitly_without_a_procedure_manifest() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"missing-procedure-manifest-test")
            .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let error = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect_err("unconfigured manifest must not produce an unbound intent");
    assert_eq!(error.code(), "procedure_manifest_unconfigured");
    assert!(!error.is_fatal());
    host.close().expect("close host");
}

#[test]
fn concurrent_fact_replacement_and_admission_are_ledger_ordered() {
    let root = TempDir::new().expect("tempdir");
    let host = Arc::new(host_with_state(
        &root,
        POLICY_INSTANCE_ALIAS,
        Arc::new(FakeState::default()),
    ));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    host.publish_fact(stored_fact(
        scope.clone(),
        "env.concurrent_authority",
        ContractFactValue::Boolean(true),
        "snapshot:concurrent-a",
        Vec::new(),
    ))
    .expect("publish initial fact revision");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let barrier = Arc::new(Barrier::new(3));

    let publisher = {
        let host = Arc::clone(&host);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            let mut replacement = stored_fact(
                scope,
                "env.concurrent_authority",
                ContractFactValue::Boolean(true),
                "snapshot:concurrent-b",
                Vec::new(),
            );
            replacement.observed_at_unix_ms += 1;
            host.publish_fact(replacement)
        })
    };
    let admitting = {
        let host = Arc::clone(&host);
        let barrier = Arc::clone(&barrier);
        let context = policy_context(&host, &intent);
        let intent = intent.clone();
        let reasons = reasons.clone();
        thread::spawn(move || {
            barrier.wait();
            host.admit_policy_dispatch(&intent, &reasons, &context)
        })
    };
    barrier.wait();
    publisher
        .join()
        .expect("publisher thread")
        .expect("replacement fact");
    let admission = admitting.join().expect("admission thread");

    let mut client = TestClient::connect(&host);
    let replacement_sequence = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .map(|event| event.sequence)
    .max()
    .expect("replacement fact sequence");
    match admission {
        Ok(PolicyDispatchAdmission::Granted { .. }) => {
            let intent_sequence = projected_events(
                &mut client,
                EventQuery {
                    event_type: Some(EventType::PolicyDispatchIntent),
                    ..EventQuery::default()
                },
            )
            .into_iter()
            .map(|event| event.sequence)
            .max()
            .expect("policy intent sequence");
            assert!(intent_sequence < replacement_sequence);
        }
        Err(error) => {
            assert_eq!(error.code(), "policy_facts_stale");
            assert!(
                projected_events(
                    &mut client,
                    EventQuery {
                        event_type: Some(EventType::PolicyDispatchIntent),
                        ..EventQuery::default()
                    }
                )
                .is_empty()
            );
        }
        Ok(other) => panic!("unexpected admission: {other:?}"),
    }
    drop(client);
    let host = Arc::try_unwrap(host).unwrap_or_else(|_| panic!("exclusive runtime host"));
    host.close().expect("close host");
}

#[test]
fn game_and_server_scope_changes_cannot_reuse_a_trusted_decision_identity() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, trusted, trusted_reasons) =
        evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let mut wrong_instance = trusted.clone();
    wrong_instance.instance_id = "fixture-instance-b".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_instance,
            &trusted_reasons,
            &policy_context(&host, &wrong_instance),
        )
        .expect_err("instance scope cannot be widened by the caller")
        .code(),
        "policy_trusted_context_mismatch"
    );

    for (index, mut facts) in [policy_facts(), policy_facts()].into_iter().enumerate() {
        if index == 0 {
            facts.instances[0].server_id = "fixture-server-b".to_owned();
        } else {
            facts.instances[0].game_id = "fixture-game-b".to_owned();
        }
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                    monotonic_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                },
                8 + index as u64,
                PolicyTrigger::Reconciliation,
            )
            .expect("scope-changed policy evaluation");
        let changed = cycle
            .evaluation
            .expect("scope-changed evaluation")
            .dispatch_intents
            .into_iter()
            .next()
            .expect("scope-changed intent");
        assert_ne!(changed.fact_snapshot_id, trusted.fact_snapshot_id);
        assert_ne!(changed.decision_id, trusted.decision_id);

        let mut mixed = changed;
        mixed.decision_id = trusted.decision_id.clone();
        mixed.reason_chain_id = trusted.reason_chain_id.clone();
        assert_eq!(
            host.admit_policy_dispatch(&mixed, &trusted_reasons, &policy_context(&host, &mixed),)
                .expect_err("scope changes cannot reuse an old decision identity")
                .code(),
            "policy_trusted_context_mismatch"
        );
    }
    host.close().expect("close host");
}

// Workflow #369 S4: policy waits (model-369-queue.md v3.1 W-1).

/// Where test (l) and test (m) refuse A's settlement at the restart.
#[derive(Clone, Copy)]
enum StartSettlementRefusal {
    /// The host's own recovery check refuses it (injected under `cfg(test)`).
    Host,
    /// The ledger refuses it (its settlement refusal hook).
    Ledger,
}

/// Coordinator ruling on #670 (safety net), test (l): one scheduled dispatch on instance A
/// whose settlement the host's recovery refuses at the restart (injected). The Runtime still
/// starts: it records `policy_settlement_dispatch_unsettled` (Error) naming the run and the
/// refusal, holds a scheduling pause of A with that reason, leaves the dispatch out of the
/// active workloads, and instance B dispatches normally.
#[test]
fn a_dispatch_the_start_cannot_settle_pauses_its_instance_and_the_other_dispatches() {
    a_start_that_cannot_settle_one_dispatch(
        StartSettlementRefusal::Host,
        "policy_execution_recovery_outcome_conflict",
    );
}

/// #670 verification N-1, test (m): as test (l), but the ledger itself refuses A's settlement
/// at the restart. The refusal is not terminal: the Runtime starts, records the Error and the
/// pause, and the ledger's writer is still there for the next appends (B's admission).
#[test]
fn a_settlement_the_ledger_refuses_at_start_leaves_the_writer_running() {
    a_start_that_cannot_settle_one_dispatch(
        StartSettlementRefusal::Ledger,
        "scheduled_execution_recovery_refused_for_test",
    );
}

fn a_start_that_cannot_settle_one_dispatch(refusal: StartSettlementRefusal, cause: &str) {
    use actingcommand_policy::SchedulingDecisionState;

    let root = TempDir::new().expect("tempdir");
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let provider = || {
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
        ]))
    };
    // Board #71: the first start's Runtime clock stands at the fixture's time and the restart's
    // at the fixed restart instant.
    let scheduled = |clock_unix_ms| {
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(clock_unix_ms, 0)))
            .with_scheduled_procedures([
                "procedure.observe".to_owned(),
                "procedure.observe-b".to_owned(),
            ])
    };
    let host = RuntimeHost::start(scheduled(POLICY_NOW_UNIX_MS), provider()).expect("first start");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("two-instance catalog activation");
    let cycle = evaluate_pending_policy(&host, PolicyTrigger::FactsChanged);
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("A's dispatch intent: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("A's reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("A's admission")
    else {
        panic!("expected A's dispatch to be granted");
    };
    let run_id = context.run_id();
    // The host's close ends A's lease (a release without run links); A's dispatch stays open.
    host.close().expect("close with A's dispatch open");

    let restart = match refusal {
        StartSettlementRefusal::Host => {
            scheduled(POLICY_RESTART_UNIX_MS).with_policy_settlement_refusal_for_test(run_id)
        }
        StartSettlementRefusal::Ledger => {
            actingcommand_ledger::refuse_scheduled_settlement_for_test(&intent.decision_id);
            scheduled(POLICY_RESTART_UNIX_MS)
        }
    };
    let host = RuntimeHost::start(restart, provider())
        .expect("a start that cannot settle one run still starts");
    assert!(host.fatal_error().expect("runtime health").is_none());
    let records = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(run_id),
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("the run's runtime failures")
        .into_iter()
        .filter(|event| {
            // Ledger revision 3 (#378 A3): the record's outcome names the refusal (its
            // `failure_code` value or its `caused_by` link) instead of message tokens.
            let outcome = match event.payload() {
                EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(record)) => {
                    Some(record.outcome())
                }
                _ => None,
            };
            event.severity() == EventSeverity::Error
                && outcome.is_some_and(|outcome| {
                    outcome.code().as_str() == "policy_settlement_dispatch_unsettled"
                        && (outcome_str(outcome, "failure_code") == Some(cause)
                            || outcome_links(
                                outcome,
                                actingcommand_contract::outcome::CauseRelation::CausedBy,
                            )
                            .iter()
                            .any(|link| link.code().as_str() == cause))
                })
        })
        .count();
    assert_eq!(records, 1, "one Error naming the run and the refusal");
    let (_, paused) = host
        .scheduling_pauses_for_test()
        .expect("scheduling pauses");
    let pause = paused
        .get(POLICY_INSTANCE_ALIAS)
        .expect("A's instance is paused");
    assert_eq!(
        pause.pause_reason.as_str(),
        "policy_settlement_dispatch_unsettled"
    );
    assert!(!paused.contains_key(POLICY_INSTANCE_ALIAS_B));

    // The restart's own records (the pause, the Error) are later than the fixture's time, so
    // the evaluation after it runs at the restart's fixed instant.
    let now = POLICY_RESTART_UNIX_MS;
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: now,
                monotonic_ms: now,
            },
            9,
            PolicyTrigger::Recovery,
        )
        .expect("the evaluation after the start");
    assert_eq!(
        cycle
            .pending_dispatch_intents
            .iter()
            .map(|intent| intent.instance_id.as_str())
            .collect::<Vec<_>>(),
        vec![POLICY_INSTANCE_ALIAS_B],
        "B dispatches; A is paused"
    );
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let deferred = evaluation
        .decisions
        .iter()
        .find(|decision| decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS))
        .expect("A's decision");
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    let intent_b = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS_B)
        .expect("B's dispatch intent")
        .clone();
    let reasons_b = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent_b.reason_chain_id)
        .expect("B's reason chain")
        .clone();
    record_policy_approval(&host, &intent_b);
    assert!(matches!(
        host.admit_policy_dispatch(&intent_b, &reasons_b, &policy_context(&host, &intent_b))
            .expect("B's admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// UNSET-1 (0.12.28; production 10-29 05:24): a dispatch an earlier start could not settle, and
// held its instance for, does not hold the instance again at every later start.

/// UNSET-1: the refusal the restarts inject for A's settlement (the host's recovery, as in
/// test (l)).
const UNSET1_REFUSAL: &str = "policy_execution_recovery_outcome_conflict";
/// UNSET-1 test 4: how long the restart's writer has to answer a read.
const UNSET1_REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// UNSET-1: A's dispatch of tests (l) and (m), open after the first host's close: the
/// instances A and B, and the run of A's dispatch.
struct OpenDispatchOnA {
    instance_a: InstanceId,
    instance_b: InstanceId,
    run_id: RunId,
}

impl OpenDispatchOnA {
    fn provider(&self) -> Arc<FakeProvider> {
        two_instance_provider(self.instance_a, self.instance_b)
    }
}

/// UNSET-1: the provider of instances A and B, each with a fresh fake state.
fn two_instance_provider(instance_a: InstanceId, instance_b: InstanceId) -> Arc<FakeProvider> {
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
    ]))
}

/// UNSET-1: the scheduled configuration of tests (l) and (m) with its Runtime clock at
/// `clock_unix_ms`.
fn unset1_config(root: &TempDir, clock_unix_ms: u64) -> RuntimeHostConfig {
    config(root)
        .with_runtime_clock(Arc::new(ManualRuntimeClock::new(clock_unix_ms, 0)))
        .with_scheduled_procedures([
            "procedure.observe".to_owned(),
            "procedure.observe-b".to_owned(),
        ])
}

/// UNSET-1: the first start of tests (l) and (m): A's dispatch admitted, then the host closed
/// with it open.
fn an_open_dispatch_on_a(root: &TempDir) -> OpenDispatchOnA {
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let host = RuntimeHost::start(
        unset1_config(root, POLICY_NOW_UNIX_MS),
        two_instance_provider(instance_a, instance_b),
    )
    .expect("first start");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("two-instance catalog activation");
    let cycle = evaluate_pending_policy(&host, PolicyTrigger::FactsChanged);
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("A's dispatch intent: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("A's reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("A's admission")
    else {
        panic!("expected A's dispatch to be granted");
    };
    let run_id = context.run_id();
    host.close().expect("close with A's dispatch open");
    OpenDispatchOnA {
        instance_a,
        instance_b,
        run_id,
    }
}

/// UNSET-1: a restart at `clock_unix_ms` whose start refuses A's settlement; `adjust` changes
/// its configuration further. The Runtime starts.
fn restart_refusing_a(
    root: &TempDir,
    open: &OpenDispatchOnA,
    clock_unix_ms: u64,
    adjust: impl FnOnce(RuntimeHostConfig) -> RuntimeHostConfig,
) -> RuntimeHost {
    let host = RuntimeHost::start(
        adjust(
            unset1_config(root, clock_unix_ms).with_policy_settlement_refusal_for_test(open.run_id),
        ),
        open.provider(),
    )
    .expect("a start that cannot settle one run still starts");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host
}

/// UNSET-1: the outcomes of the run's `runtime.failed` records of `code` at `severity`, in
/// ledger order.
fn unset1_records(
    host: &RuntimeHost,
    run_id: RunId,
    severity: EventSeverity,
    code: &str,
) -> Vec<actingcommand_contract::outcome::Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        run_id: Some(run_id),
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("the run's runtime failures")
    .into_iter()
    .filter(|event| event.severity() == severity)
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(record))
            if record.outcome().code().as_str() == code =>
        {
            Some(record.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// UNSET-1: the run's `policy_settlement_dispatch_unsettled` records (Error).
fn unsettled_errors(
    host: &RuntimeHost,
    run_id: RunId,
) -> Vec<actingcommand_contract::outcome::Outcome> {
    unset1_records(
        host,
        run_id,
        EventSeverity::Error,
        "policy_settlement_dispatch_unsettled",
    )
}

/// UNSET-1: the run's `policy_settlement_dispatch_left_open` records (Warning).
fn left_open_warnings(
    host: &RuntimeHost,
    run_id: RunId,
) -> Vec<actingcommand_contract::outcome::Outcome> {
    unset1_records(
        host,
        run_id,
        EventSeverity::Warning,
        "policy_settlement_dispatch_left_open",
    )
}

/// UNSET-1: the outcome's `note` links.
fn note_links(
    outcome: &actingcommand_contract::outcome::Outcome,
) -> Vec<&actingcommand_contract::outcome::Outcome> {
    outcome_links(
        outcome,
        actingcommand_contract::outcome::CauseRelation::Note,
    )
}

/// UNSET-1: A's scheduling pause, if any.
fn pause_of_a(host: &RuntimeHost) -> Option<actingcommand_contract::InstancePauseState> {
    let (_, paused) = host
        .scheduling_pauses_for_test()
        .expect("scheduling pauses");
    paused.get(POLICY_INSTANCE_ALIAS).cloned()
}

/// UNSET-1: a person lifts A's pause after looking (`actingctl resume` of A).
fn resume_a(host: &RuntimeHost) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::ResumeScheduling {
                    scope: actingcommand_contract::SchedulingPauseScope::Instance {
                        instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
                    },
                    expected: None,
                },
            ),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("resume receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(pause_of_a(host).is_none(), "A's pause is lifted");
}

/// UNSET-1 test 1: the first restart that cannot settle A's dispatch holds as before: one Error
/// naming the run and the refusal, without a note (the run's records were read), and A paused
/// with that code as its reason.
fn first_hold_of_a(root: &TempDir, open: &OpenDispatchOnA) -> RuntimeHost {
    let host = restart_refusing_a(root, open, POLICY_RESTART_UNIX_MS, |config| config);
    let errors = unsettled_errors(&host, open.run_id);
    assert_eq!(errors.len(), 1, "one Error naming the run: {errors:#?}");
    assert_eq!(
        outcome_str(&errors[0], "failure_code"),
        Some(UNSET1_REFUSAL)
    );
    assert!(note_links(&errors[0]).is_empty(), "{errors:#?}");
    assert!(left_open_warnings(&host, open.run_id).is_empty());
    let pause = pause_of_a(&host).expect("A's instance is paused");
    assert_eq!(
        pause.pause_reason.as_str(),
        "policy_settlement_dispatch_unsettled"
    );
    host
}

/// UNSET-1 test 2 (test 1 at its first restart): after a person lifted A's first pause, a later
/// start that refuses the same run records `policy_settlement_dispatch_left_open` (Warning) with
/// the refusal as its `failure_code`, records no second Error, and does not pause A.
#[test]
fn unset1_a_later_start_after_the_lifted_hold_records_left_open_without_a_pause() {
    let root = TempDir::new().expect("tempdir");
    let open = an_open_dispatch_on_a(&root);
    let host = first_hold_of_a(&root, &open);
    resume_a(&host);
    host.close().expect("close after the resume");

    let host = restart_refusing_a(&root, &open, POLICY_RESTART_UNIX_MS + 60_000, |config| {
        config
    });
    assert_eq!(
        unsettled_errors(&host, open.run_id).len(),
        1,
        "no second Error"
    );
    let left_open = left_open_warnings(&host, open.run_id);
    assert_eq!(left_open.len(), 1, "one Warning: {left_open:#?}");
    assert_eq!(
        outcome_str(&left_open[0], "failure_code"),
        Some(UNSET1_REFUSAL)
    );
    assert_eq!(
        outcome_str(&left_open[0], "operation"),
        Some("hold_unsettled_dispatches")
    );
    assert!(pause_of_a(&host).is_none(), "A is not paused again");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// UNSET-1 test 3: with A's first pause still in place, a later start that refuses the same run
/// records the Warning and no second Error, and adds no second pause: A's pause is the first
/// one, restored.
#[test]
fn unset1_a_later_start_under_the_first_pause_adds_no_second_pause() {
    let root = TempDir::new().expect("tempdir");
    let open = an_open_dispatch_on_a(&root);
    let host = first_hold_of_a(&root, &open);
    let first = pause_of_a(&host).expect("A's first pause");
    host.close().expect("close with A paused");

    let host = restart_refusing_a(&root, &open, POLICY_RESTART_UNIX_MS + 60_000, |config| {
        config
    });
    assert_eq!(
        unsettled_errors(&host, open.run_id).len(),
        1,
        "no second Error"
    );
    let left_open = left_open_warnings(&host, open.run_id);
    assert_eq!(left_open.len(), 1, "one Warning: {left_open:#?}");
    assert_eq!(
        outcome_str(&left_open[0], "failure_code"),
        Some(UNSET1_REFUSAL)
    );
    let pause = pause_of_a(&host).expect("A's first pause is still in place");
    assert_eq!(pause.pause_reason.as_str(), first.pause_reason.as_str());
    assert_eq!(
        pause.since_unix_ms, first.since_unix_ms,
        "the first pause, restored; no second pause"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// UNSET-1 test 4: the read hold armed right before the start's read of the run's records and
/// released once that read has returned; whether it held a read.
struct HeldUnsettledRecordRead {
    gate: Option<actingcommand_ledger::WriterReadHoldGate>,
    held: Arc<AtomicBool>,
}

impl Drop for HeldUnsettledRecordRead {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.take() {
            self.held
                .store(gate.wait_held(Duration::ZERO), Ordering::SeqCst);
            gate.release();
        }
    }
}

/// UNSET-1 test 4: installs on this thread the read hold of `root` around the next start's read
/// of an unsettled dispatch's run records; whether it held the read, once it has returned.
fn hold_the_unsettled_record_read(root: &Path) -> Arc<AtomicBool> {
    let held = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&held);
    let state_root = root.to_path_buf();
    crate::host::AROUND_UNSETTLED_RECORD_READ.with(|around| {
        *around.borrow_mut() = Some(Box::new(move || -> Box<dyn std::any::Any> {
            Box::new(HeldUnsettledRecordRead {
                gate: Some(
                    actingcommand_ledger::WriterReadHoldGate::arm(&state_root)
                        .expect("arm the read hold"),
                ),
                held: flag,
            })
        }));
    });
    held
}

/// UNSET-1 test 4: after a person lifted A's first pause, a later start whose read of the run's
/// records fails (the writer does not answer it within the reply limit; a read stopped at its
/// budget fails the same way) holds as before: a second Error naming the refusal, whose `note`
/// link is the read's failure, no Warning, and A paused again.
#[test]
fn unset1_a_failed_read_of_the_run_records_holds_as_before_and_says_why() {
    let root = TempDir::new().expect("tempdir");
    let open = an_open_dispatch_on_a(&root);
    let host = first_hold_of_a(&root, &open);
    resume_a(&host);
    host.close().expect("close after the resume");

    let held = hold_the_unsettled_record_read(root.path());
    let host = restart_refusing_a(&root, &open, POLICY_RESTART_UNIX_MS + 60_000, |config| {
        config.with_ledger_reply_timeout(UNSET1_REPLY_TIMEOUT)
    });
    assert!(
        held.load(Ordering::SeqCst),
        "the read of the run's records was held past the reply limit"
    );
    let errors = unsettled_errors(&host, open.run_id);
    assert_eq!(errors.len(), 2, "the start holds as before: {errors:#?}");
    let noted = errors
        .iter()
        .filter(|outcome| !note_links(outcome).is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        noted.len(),
        1,
        "only the second Error says why: {errors:#?}"
    );
    assert_eq!(outcome_str(noted[0], "failure_code"), Some(UNSET1_REFUSAL));
    assert!(
        note_links(noted[0]).iter().any(|note| {
            note.code().as_str() == "writer_response_timeout"
                && outcome_str(note, "operation") == Some("read_unsettled_dispatch_records")
        }),
        "the note is the read's failure: {noted:#?}"
    );
    assert!(left_open_warnings(&host, open.run_id).is_empty());
    let pause = pause_of_a(&host).expect("A is paused again");
    assert_eq!(
        pause.pause_reason.as_str(),
        "policy_settlement_dispatch_unsettled"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// UNSET-2 (0.12.51, register §1.296; production 10-26): a scheduled run cut by a stop after its
// own failure path released its lease, with an effect record and without a terminal, which an
// earlier start refused to settle, is settled once by the next start, which records who closed
// it and why; the late settlement leaves its pair's failure state as it was. Fix round 1 (the
// coordinator's rulings L1 to L4): a pair's latest completed run is the latest in admission
// order, so a late settlement never stands for a pair that ran after it, and it is not a failure
// of its pair anywhere; every start after the settling one starts.

/// UNSET-2: evaluates at `now` with `seed` and admits A's dispatch: its run context and its
/// intent.
fn unset2_admit_a_at(
    host: &RuntimeHost,
    now: u64,
    seed: u64,
) -> (Box<PolicyRunContext>, DispatchIntent) {
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: now,
                monotonic_ms: now,
            },
            seed,
            PolicyTrigger::Recovery,
        )
        .expect("the evaluation");
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .unwrap_or_else(|| panic!("A's dispatch intent: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("A's reason chain")
        .clone();
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
        .expect("A's admission")
    else {
        panic!("expected A's dispatch to be granted");
    };
    (context, intent)
}

/// UNSET-2: an effect record of the run of `context` is written; then the run's own failure path
/// releases its lease (effect `performed`) and the stop cuts it before its execution record (the
/// injected append failure latches the Runtime, as a crash ends it), and `host` is closed. The
/// run has its own release, an effect record and no terminal.
fn unset2_cut_after_its_own_release(host: RuntimeHost, context: &PolicyRunContext) {
    host.append_contained_task_semantic_for_test(
        context.request(),
        context.lease_token(),
        context.issued_task_id(),
        context.issued_run_id(),
        TaskSemanticFact::EffectIntent {
            frame_extent: None,
            step_index: 0,
            operation_label: "fixture".to_owned(),
            action: InputAction::Reset,
        },
    )
    .expect("the run's effect record");
    host.fail_next_policy_execution_append_for_test()
        .expect("arm the execution append failure");
    let error = host
        .complete_scheduled_policy_failure_without_terminal_for_test(context)
        .expect_err("the stop cuts the run before its execution record");
    assert_eq!(error.code(), "ledger_failure");
    let releases = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            event_type: Some(EventType::LeaseReleased),
            ..EventQuery::default()
        })
        .expect("the run's releases");
    assert_eq!(releases.len(), 1, "the run's own release: {releases:#?}");
    if let Err(error) = host.close() {
        assert_eq!(error.code(), "ledger_failure", "{error:?}");
    }
}

/// UNSET-2: the first start under the two-instance catalog `sources` (the pending one, or a
/// variant of it): A's dispatch admitted, its run cut after its own release
/// ([`unset2_cut_after_its_own_release`]). Returns the open dispatch on A and its intent.
fn a_dispatch_on_a_cut_after_its_own_release(
    root: &TempDir,
    sources: &CatalogSources,
) -> (OpenDispatchOnA, DispatchIntent) {
    let (instance_a, instance_b) = (instance_id(), instance_id());
    let host = RuntimeHost::start(
        unset1_config(root, POLICY_NOW_UNIX_MS),
        two_instance_provider(instance_a, instance_b),
    )
    .expect("first start");
    host.activate_policy_catalog(sources)
        .expect("two-instance catalog activation");
    let (context, intent) = unset2_admit_a_at(&host, POLICY_NOW_UNIX_MS, 7);
    let run_id = context.run_id();
    unset2_cut_after_its_own_release(host, &context);
    (
        OpenDispatchOnA {
            instance_a,
            instance_b,
            run_id,
        },
        intent,
    )
}

/// UNSET-2: a restart at `clock_unix_ms` with no refusal injected. The Runtime starts.
fn unset2_restart(root: &TempDir, open: &OpenDispatchOnA, clock_unix_ms: u64) -> RuntimeHost {
    let host = RuntimeHost::start(unset1_config(root, clock_unix_ms), open.provider())
        .expect("the restart");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host
}

/// UNSET-2: the number of `event_type` records (executions or completions) of `decision_id`.
fn unset2_decision_records(host: &RuntimeHost, event_type: EventType, decision_id: &str) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("the policy records")
    .iter()
    .filter(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
            payload.decision_id() == decision_id
        }
        EventPayload::Policy(PolicyPayload::DispatchCompleted(payload)) => {
            payload.decision_id() == decision_id
        }
        _ => false,
    })
    .count()
}

/// UNSET-2: the run's `policy_settlement_dispatch_closed` records (Info).
fn closed_records(
    host: &RuntimeHost,
    run_id: RunId,
) -> Vec<actingcommand_contract::outcome::Outcome> {
    unset1_records(
        host,
        run_id,
        EventSeverity::Info,
        "policy_settlement_dispatch_closed",
    )
}

/// UNSET-2: the value record of the pair of `task_id` on the instance `instance_alias` (its
/// failure state and the dispatches its journal names among it), held and replayed the same.
fn unset2_pair_record(
    host: &RuntimeHost,
    instance_alias: &str,
    task_id: &str,
) -> actingcommand_contract::PolicyControlStateRecord {
    let (held, replayed, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    let key = format!("decision_memory/{instance_alias}/{task_id}");
    let record =
        |records: &BTreeMap<String, crate::policy_control::PolicyValueRecord>| match records
            .get(&key)
        {
            Some(crate::policy_control::PolicyValueRecord::Control(record)) => (**record).clone(),
            other => panic!("A's pair's value record: {other:?}"),
        };
    assert_eq!(record(&held), record(&replayed), "held and replayed");
    record(&held)
}

/// UNSET-2: the settling start's records of the run `run_id` of `decision_id`: the run settled
/// once (one execution record, one completion), one `policy_settlement_dispatch_closed` naming
/// the start's operation and the earlier refusal, the one Error of the earlier start and no
/// Warning.
fn assert_settled_once_and_closed(host: &RuntimeHost, run_id: RunId, decision_id: &str) {
    assert_eq!(
        unset2_decision_records(host, EventType::PolicyExecutionRecorded, decision_id),
        1,
        "one execution record"
    );
    assert_eq!(
        unset2_decision_records(host, EventType::PolicyDispatchCompleted, decision_id),
        1,
        "one completion"
    );
    let closed = closed_records(host, run_id);
    assert_eq!(closed.len(), 1, "one record of who closed it: {closed:#?}");
    assert_eq!(
        outcome_str(&closed[0], "failure_code"),
        Some(UNSET1_REFUSAL)
    );
    assert_eq!(
        outcome_str(&closed[0], "operation"),
        Some("reconcile_policy_outcomes")
    );
    assert_eq!(unsettled_errors(host, run_id).len(), 1);
    assert!(left_open_warnings(host, run_id).is_empty());
}

/// UNSET-2 fix 1 (ruling L2): `after`'s failure state is `before`'s: its failure streak (with its
/// pause or backoff), its retry round, the place of its latest failure-setting settlement and its
/// owed reruns.
fn assert_failure_state_kept(
    before: &actingcommand_contract::PolicyControlStateRecord,
    after: &actingcommand_contract::PolicyControlStateRecord,
    at: &str,
) {
    assert_eq!(
        after.failure_streak, before.failure_streak,
        "{at}: the failure streak, its pause or backoff"
    );
    assert_eq!(
        after.retry_round, before.retry_round,
        "{at}: the retry round"
    );
    assert_eq!(
        after.failure_settled, before.failure_settled,
        "{at}: the latest failure-setting settlement"
    );
    assert_eq!(
        after.condition_reruns, before.condition_reruns,
        "{at}: the owed reruns"
    );
}

/// UNSET-2 fix 1: the task snapshot of the pair of `task_id` on `instance_alias`, as the
/// evaluation at `now` projects it from the dispatch journal.
fn unset2_snapshot(
    host: &RuntimeHost,
    instance_alias: &str,
    task_id: &str,
    now: u64,
) -> actingcommand_policy::TaskRuntimeSnapshot {
    host.policy_task_snapshots_for_test(now)
        .expect("the task snapshots")
        .into_iter()
        .find(|snapshot| snapshot.task_id == task_id && snapshot.instance_id == instance_alias)
        .expect("the pair's snapshot")
}

/// UNSET-2 fix 1: the number of `runtime.fact_recorded` rows of the settlement facts of
/// `task_id` (`task.<task>.*`). A start that derives the same facts appends none.
fn unset2_settlement_fact_rows(host: &RuntimeHost, task_id: &str) -> usize {
    let prefix = format!("task.{task_id}.");
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFactRecorded),
        ..EventQuery::default()
    })
    .expect("the fact records")
    .iter()
    .filter(|event| {
        matches!(
            event.payload(),
            EventPayload::Runtime(actingcommand_contract::RuntimePayload::FactRecorded(payload))
                if payload.record().key.starts_with(&prefix)
        )
    })
    .count()
}

/// UNSET-2 fix 1: the journal entries a pair's value record named before the settlement that are
/// completed (the later run, in R), which the record names alone after it.
fn unset2_completed_entries(
    record: &actingcommand_contract::PolicyControlStateRecord,
) -> Vec<actingcommand_contract::PolicyJournalDispatchRecord> {
    record
        .dispatches
        .iter()
        .filter(|dispatch| dispatch.completed_sequence.is_some())
        .copied()
        .collect()
}

/// UNSET-2 test 1 (the pair has not run since): the first restart refuses the run's settlement
/// and holds A (UNSET-1). The next start settles it once and records who closed it and why; the
/// late settlement leaves the pair's failure state as it was before it (ruling L2): no failure
/// streak, pause or backoff, as before, and the retry round the cut run's own admission opened,
/// as before; at the settling start and at the next one. A later start writes neither a
/// settlement nor a Warning for it, and the pair's eligibility age runs again.
#[test]
fn unset2_the_next_start_settles_a_run_cut_after_its_own_release_once() {
    let root = TempDir::new().expect("tempdir");
    let (open, intent) =
        a_dispatch_on_a_cut_after_its_own_release(&root, &pending_policy_sources(1));
    let host = first_hold_of_a(&root, &open);
    let before = unset2_pair_record(&host, &intent.instance_id, &intent.task_id);
    assert!(before.failure_streak.is_none(), "{before:#?}");
    // Fix round 2: a normal admission opens its pair's retry round (`commit_admission`), so the
    // cut run's round is part of the pair's failure state before the settlement.
    assert_eq!(
        before
            .retry_round
            .as_ref()
            .map(|round| round.admission.activity.admitted_at_unix_ms),
        Some(POLICY_NOW_UNIX_MS),
        "the cut run's retry round: {before:#?}"
    );
    host.close().expect("close with A held");

    let host = unset2_restart(&root, &open, POLICY_RESTART_UNIX_MS + 60_000);
    assert_settled_once_and_closed(&host, open.run_id, &intent.decision_id);
    let after = unset2_pair_record(&host, &intent.instance_id, &intent.task_id);
    assert_failure_state_kept(&before, &after, "the settling start");
    host.close().expect("close after the settlement");

    let later = POLICY_RESTART_UNIX_MS + 120_000;
    let host = unset2_restart(&root, &open, later);
    assert_settled_once_and_closed(&host, open.run_id, &intent.decision_id);
    let next = unset2_pair_record(&host, &intent.instance_id, &intent.task_id);
    assert_failure_state_kept(&before, &next, "the next start");
    resume_a(&host);
    let evaluate = |unix_ms: u64, seed: u64, trigger: PolicyTrigger| {
        host.evaluate_policy_cycle_with_test_inputs(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            trigger,
        )
        .expect("an evaluation after the settlement")
    };
    let first = evaluate(later, 9, PolicyTrigger::Recovery);
    assert!(first.evaluation.is_some(), "the first evaluation ran");
    let second = evaluate(later + 67_000, 10, PolicyTrigger::Reconciliation);
    assert!(second.evaluation.is_some(), "the second evaluation ran");
    let snapshot = host
        .policy_task_snapshots_for_test(later + 67_000)
        .expect("the task snapshots")
        .into_iter()
        .find(|snapshot| {
            snapshot.task_id == intent.task_id && snapshot.instance_id == intent.instance_id
        })
        .expect("A's pair");
    assert_eq!(
        snapshot.eligible_since_unix_ms,
        Some(later),
        "the pair's eligibility age runs again"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// UNSET-2 fix 1 (R): the cut run's dispatch, open on A; the cut run's intent and the later
/// run's; the pair's value record and its settlement fact rows after the later run, before the
/// settlement.
struct LaterRunOfA {
    open: OpenDispatchOnA,
    cut: DispatchIntent,
    later: DispatchIntent,
    before: actingcommand_contract::PolicyControlStateRecord,
    fact_rows: usize,
}

/// UNSET-2 fix 1 (R): under the catalog `sources`, A's run is cut after its own release; the
/// first restart holds A (UNSET-1) and a person lifts the hold; the pair runs again under a new
/// decision, which succeeds, or fails when `fails`, while the cut run stays open; the host is
/// closed.
fn a_later_run_while_the_cut_run_is_open(
    root: &TempDir,
    sources: &CatalogSources,
    fails: bool,
) -> LaterRunOfA {
    let (open, cut) = a_dispatch_on_a_cut_after_its_own_release(root, sources);
    let host = first_hold_of_a(root, &open);
    resume_a(&host);
    let (context, later) = unset2_admit_a_at(&host, POLICY_RESTART_UNIX_MS, 9);
    assert_ne!(later.decision_id, cut.decision_id);
    if fails {
        host.complete_scheduled_policy_failure_without_terminal_for_test(&context)
            .expect("the later run fails");
    } else {
        host.complete_scheduled_policy_success_without_terminal_for_test(&context)
            .expect("the later run succeeds");
    }
    drop(context);
    let before = unset2_pair_record(&host, &cut.instance_id, &cut.task_id);
    let fact_rows = unset2_settlement_fact_rows(&host, &cut.task_id);
    host.close().expect("close after the later run");
    LaterRunOfA {
        open,
        cut,
        later,
        before,
        fact_rows,
    }
}

/// UNSET-2 fix 1, at one start after the late settlement in R: the pair's failure state is as it
/// was before the settlement; its value record names the later run alone, with the later run's
/// own failure count `failures`; its task snapshot reads the later run (`terminal`); and no
/// settlement fact was recorded since. A loading start (`loaded`) holds the later run, not the
/// late one, in its dispatch journal.
fn assert_the_later_run_stands(
    host: &RuntimeHost,
    r: &LaterRunOfA,
    (now, start): (u64, &str),
    terminal: actingcommand_policy::TaskTerminalState,
    failures: u64,
    loaded: bool,
) {
    assert_settled_once_and_closed(host, r.open.run_id, &r.cut.decision_id);
    let after = unset2_pair_record(host, &r.cut.instance_id, &r.cut.task_id);
    assert_failure_state_kept(&r.before, &after, start);
    let named = unset2_completed_entries(&r.before);
    assert_eq!(named.len(), 1, "{start}: the later run: {:#?}", r.before);
    assert_eq!(
        named[0].failures,
        Some(failures),
        "{start}: the later run's own count"
    );
    assert_eq!(
        after.dispatches, named,
        "{start}: the value record names the later run alone"
    );
    let snapshot = unset2_snapshot(host, &r.cut.instance_id, &r.cut.task_id, now);
    assert_eq!(
        snapshot.last_dispatched_unix_ms,
        Some(POLICY_RESTART_UNIX_MS),
        "{start}: the pair's latest run is the later one"
    );
    assert_eq!(snapshot.terminal_state, Some(terminal), "{start}");
    assert_eq!(
        snapshot.failure_streak,
        Some(u16::try_from(failures).expect("a small count")),
        "{start}: the later run's own count"
    );
    assert_eq!(
        unset2_settlement_fact_rows(host, &r.cut.task_id),
        r.fact_rows,
        "{start}: the pair's settlement facts are unchanged"
    );
    if loaded {
        let (resident, _) = host
            .dispatch_journal_for_test()
            .expect("the dispatch journal");
        assert_eq!(
            resident.0.get(&r.later.decision_id),
            Some(&("Completed".to_owned(), Some(failures))),
            "{start}: the loaded journal holds the later run"
        );
        assert!(
            !resident.0.contains_key(&r.cut.decision_id),
            "{start}: and not the late one: {resident:#?}"
        );
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
}

/// UNSET-2 fix 1 test 1 (R, ruling L4): after the first restart held A and a person lifted the
/// hold, the pair ran again and succeeded while the cut run stayed open. The next start settles
/// the cut run late; it and two more starts start. At each the pair's settlement facts are
/// unchanged, its task snapshot and value record read the later run, its failure state is what
/// it was before the settlement, and a loading start's journal holds the later run.
#[test]
fn unset2_r_the_starts_after_a_late_settlement_start_and_read_the_later_run() {
    let root = TempDir::new().expect("tempdir");
    let r = a_later_run_while_the_cut_run_is_open(&root, &pending_policy_sources(1), false);
    assert!(r.before.failure_streak.is_none(), "{:#?}", r.before);
    for (offset, start) in [
        (60_000, "the settling start"),
        (120_000, "the next start"),
        (180_000, "the start after it"),
    ] {
        let now = POLICY_RESTART_UNIX_MS + offset;
        let host = unset2_restart(&root, &r.open, now);
        assert_the_later_run_stands(
            &host,
            &r,
            (now, start),
            actingcommand_policy::TaskTerminalState::Succeeded,
            0,
            offset > 60_000,
        );
        assert!(pause_of_a(&host).is_none(), "{start}: A is not paused");
        host.close().expect("close host");
    }
}

/// UNSET-2 fix 1 test 3 (R, ruling L2): as test 1, but the later run failed. Its failure streak
/// with its backoff and its retry round are kept, and the pair's failure count is the later
/// run's own (1), at the settling start and at the next one.
#[test]
fn unset2_r_a_later_failure_keeps_its_backoff_and_its_own_count() {
    let root = TempDir::new().expect("tempdir");
    let r = a_later_run_while_the_cut_run_is_open(&root, &pending_policy_sources(1), true);
    assert!(
        r.before.failure_streak.is_some(),
        "the later run's failure: {:#?}",
        r.before
    );
    for (offset, start) in [(60_000, "the settling start"), (120_000, "the next start")] {
        let now = POLICY_RESTART_UNIX_MS + offset;
        let host = unset2_restart(&root, &r.open, now);
        assert_the_later_run_stands(
            &host,
            &r,
            (now, start),
            actingcommand_policy::TaskTerminalState::Failed,
            1,
            offset > 60_000,
        );
        host.close().expect("close host");
    }
}

/// UNSET-2 fix 2: the pending catalog whose task on A has no retry and pauses at its first
/// failure (`on_failure` `retry_limit` 0, `action` `pause`), so that a failed run of the pair is
/// one `actingd suspended` lists, with its count.
fn unset2_pausing_sources() -> CatalogSources {
    let mut sources = pending_policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("the pending tasks");
    tasks["tasks"][0]["on_failure"]["retry_limit"] = serde_json::json!(0);
    tasks["tasks"][0]["on_failure"]["action"] = serde_json::json!("pause");
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the pending task bytes");
    sources
}

/// UNSET-2 fix 1 test 5 (R, rulings L1 and L2; fix round 2, the second review's §7): the pair's
/// later run failed and paused its task (a catalog whose task pauses at its first failure).
/// After the settling start, `actingd suspended` lists the pair once, as suspended by its later
/// run with that run's own count of the same error in a row (1). The late interruption is not
/// the pair's latest failure: the pair is not listed as lifted by the restart, as the late
/// interruption was before fix round 1, nor as repeating.
#[test]
fn unset2_r_actingd_suspended_reports_the_later_run_not_the_late_interruption() {
    let root = TempDir::new().expect("tempdir");
    let sources = unset2_pausing_sources();
    let r = a_later_run_while_the_cut_run_is_open(&root, &sources, true);
    let held = r
        .before
        .failure_streak
        .as_ref()
        .unwrap_or_else(|| panic!("the later run's failure: {:#?}", r.before));
    assert_eq!(
        (held.disposition, held.count, held.decision_id.as_str()),
        (
            PolicyFailureDisposition::PausedTask,
            1,
            r.later.decision_id.as_str()
        ),
        "the later run paused its task: {:#?}",
        r.before
    );
    let host = unset2_restart(&root, &r.open, POLICY_RESTART_UNIX_MS + 60_000);
    assert_settled_once_and_closed(&host, r.open.run_id, &r.cut.decision_id);
    host.close().expect("close after the settlement");
    let report = crate::suspension_report(&crate::SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &unset1_config(&root, POLICY_RESTART_UNIX_MS + 60_000),
        catalog: &sources,
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    let listed = |list: &str| -> Vec<serde_json::Value> {
        report[list]
            .as_array()
            .unwrap_or_else(|| panic!("{list}: {report:#}"))
            .iter()
            .filter(|entry| {
                entry["task_id"] == r.cut.task_id.as_str()
                    && entry["instance_id"] == r.cut.instance_id.as_str()
            })
            .cloned()
            .collect()
    };
    let suspended = listed("suspended");
    assert_eq!(
        suspended.len(),
        1,
        "the pair is suspended by its later run: {report:#}"
    );
    assert_eq!(
        suspended[0]["paused"]["decision_id"],
        r.later.decision_id.as_str(),
        "the later run, not the late one: {report:#}"
    );
    assert_eq!(
        suspended[0]["consecutive_same_error"], 1,
        "the later run's own count: {report:#}"
    );
    for list in ["lifted", "repeating"] {
        assert!(
            listed(list).is_empty(),
            "{list} lists the pair, whose latest failure is its later run's: {report:#}"
        );
    }
}

/// UNSET-2 fix 1: the outcome key the mapped fixture's task gives and its consumer reads.
const UNSET2_OUTCOME_KEY: &str = "unset2-fixture-result";

/// UNSET-2 fix 1: `config` with the fixture's procedures as scheduled runs, as UNSET-1's are, so
/// a start settles a run cut before its first record.
fn unset2_scheduled(config: RuntimeHostConfig) -> RuntimeHostConfig {
    config.with_scheduled_procedures([
        "procedure.observe".to_owned(),
        "procedure.observe-b".to_owned(),
    ])
}

/// UNSET-2 fix 2: how the mapped fixture's instance A is backed. A person's resume of an
/// instance (`actingctl resume`, `ResumeScheduling` of an instance scope) applies to a
/// device-backed instance only (Workflow #191 ps1: `require_physical_instance_alias` refuses any
/// other with `fixture_execution_scope_forbidden`), so a test that lifts UNSET-1's hold runs A on
/// a device, as UNSET-1's tests do; a test that never lifts it keeps the mapped fixture's
/// simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unset2Backend {
    /// The mapped fixture's simulation (`admitted_mapped_run_fixture_configured`).
    Fixture,
    /// A device with the fake's task geometry, which runs the device twin of the mapped package
    /// (`neutral_two_key_mapped_contained_task_package` with task geometry).
    Device,
}

/// UNSET-2 fix 1: a start of the mapped fixture's Runtime over `root` at the clock's time:
/// instance A on `backend`, whose task `fixture.observe` gives the outcome its consumer
/// `fixture.followup` reads. `refuse` is the run whose settlement the start refuses, as UNSET-1's
/// hold. The Runtime starts.
fn unset2_mapped_start(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    instance: InstanceId,
    refuse: Option<RunId>,
    backend: Unset2Backend,
) -> RuntimeHost {
    let config = unset2_scheduled(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts(UNSET2_OUTCOME_KEY, false),
                policy_resources(),
            ))
            .with_procedure_manifest(procedure_manifest_with_primary(
                b"",
                vec!["after_observation".to_owned()],
            )),
    );
    let config = match refuse {
        Some(run_id) => config.with_policy_settlement_refusal_for_test(run_id),
        None => config,
    };
    let provider = FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(state));
    let provider = match backend {
        Unset2Backend::Fixture => provider.fixture_simulation(),
        Unset2Backend::Device => provider,
    };
    let host = RuntimeHost::start(config, Arc::new(provider)).expect("the mapped fixture's start");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host
}

/// UNSET-2 fix 1: moves the Runtime clock one second on; the new time.
fn unset2_tick(clock: &ManualRuntimeClock) -> u64 {
    let now = clock.sample().expect("the Runtime clock").unix_ms + 1_000;
    advance_manual_clock_to(clock, now);
    now
}

/// UNSET-2 fix 1: the first restart of the mapped fixture on `backend` after the run `cut` was
/// cut: it refuses the run's settlement and holds A once (UNSET-1).
fn unset2_mapped_hold(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    instance: InstanceId,
    cut: RunId,
    backend: Unset2Backend,
) -> RuntimeHost {
    unset2_tick(clock);
    let host = unset2_mapped_start(root, clock, state, instance, Some(cut), backend);
    assert_eq!(unsettled_errors(&host, cut).len(), 1, "the first hold");
    assert!(pause_of_a(&host).is_some(), "A is held");
    host
}

/// UNSET-2 fix 1 test 2: whether the evaluation at `unix_ms` dispatches the consumer of A's
/// task's outcome (`fixture.followup`, which runs while that outcome is true).
fn unset2_consumer_dispatches(
    host: &RuntimeHost,
    unix_ms: u64,
    seed: u64,
    trigger: PolicyTrigger,
) -> bool {
    host.evaluate_policy_cycle_with_test_inputs(
        &mapped_policy_facts_at(UNSET2_OUTCOME_KEY, unix_ms, false),
        &policy_resources(),
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
        seed,
        trigger,
    )
    .expect("the consumer's evaluation")
    .evaluation
    .expect("an evaluation that ran")
    .dispatch_intents
    .iter()
    .any(|intent| intent.task_id == "fixture.followup")
}

/// UNSET-2 fix 1 test 2 (R with a referenced outcome key, ruling L1): A's task's outcome is read
/// by a consumer. A's run is cut after its own release and held; after a person lifts the hold
/// the pair runs again and succeeds, its terminal giving the outcome, so the consumer
/// dispatches. The settling start and the next one start, the later run stands for the pair in
/// the outcome-key snapshot, and the consumer decides as before the settlement. Fix round 2: A is
/// a device (`Unset2Backend::Device`), as in UNSET-1's tests, since a person's resume applies to
/// a device-backed instance only; the later run runs the device twin of the mapped package.
#[test]
fn unset2_r_a_rule_on_the_task_outcome_decides_after_the_settlement_as_before() {
    let root = TempDir::new().expect("tempdir");
    let device = Unset2Backend::Device;
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let package_path = root.path().join("unset2-device-mapped-task.zip");
    fs::write(
        &package_path,
        neutral_two_key_mapped_contained_task_package(
            true,
            UNSET2_OUTCOME_KEY,
            &complementary_outcome_key(UNSET2_OUTCOME_KEY),
        ),
    )
    .expect("write the mapped package");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the mapped package request");
    let instance = instance_id();
    let host = unset2_mapped_start(&root, &clock, &state, instance, None, device);
    host.activate_policy_catalog(&mapped_policy_sources(1, UNSET2_OUTCOME_KEY))
        .expect("activate the mapped catalog");
    let (cut, _) = admit_mapped_run_at(&host, UNSET2_OUTCOME_KEY, POLICY_NOW_UNIX_MS, 42_100);
    let cut_run = cut.run_id();
    unset2_cut_after_its_own_release(host, &cut);
    let host = unset2_mapped_hold(&root, &clock, &state, instance, cut_run, device);
    resume_a(&host);
    let at = cut
        .admission()
        .activity
        .next_eligible_unix_ms
        .max(POLICY_NOW_UNIX_MS + MAPPED_RUN_MIN_ADVANCE_MS);
    advance_manual_clock_to(&clock, at);
    let (later, _) = admit_mapped_run_at(&host, UNSET2_OUTCOME_KEY, at, 42_101);
    let receipt = host
        .run_scheduled_contained_task(&later, &request)
        .expect("the later run");
    host.complete_scheduled_policy_run(&later, &receipt)
        .expect("the later run's completion");
    let pair = (
        later.catalog_task_id().to_owned(),
        later.instance_alias().to_owned(),
    );
    let after_run = later
        .admission()
        .activity
        .next_eligible_unix_ms
        .max(clock.sample().expect("the Runtime clock").unix_ms);
    advance_manual_clock_to(&clock, after_run);
    assert!(
        unset2_consumer_dispatches(&host, after_run, 42_102, PolicyTrigger::FactsChanged),
        "before the settlement the consumer reads the later run's outcome"
    );
    host.close().expect("close after the later run");

    for (start, seed) in [("the settling start", 42_110), ("the next start", 42_111)] {
        let now = unset2_tick(&clock);
        let host = unset2_mapped_start(&root, &clock, &state, instance, None, device);
        assert_settled_once_and_closed(&host, cut_run, cut.decision_id());
        let snapshot = host
            .policy_outcome_key_snapshot_for_test()
            .expect("the outcome-key snapshot");
        assert_eq!(
            snapshot.completed_runs.get(&pair).map(|run| run.run_id),
            Some(later.run_id()),
            "{start}: the later run stands for the pair"
        );
        assert!(
            unset2_consumer_dispatches(&host, now, seed, PolicyTrigger::Recovery),
            "{start}: the consumer decides as before the settlement"
        );
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close host");
    }
}

/// UNSET-2 fix 1 test 4 (N, rulings L2 and L3): A's task's outcome is read by a consumer. A's
/// run is cut after its own release and held, and the pair does not run again; in the second
/// case the pair's failure before the cut run was itself an interruption (its first in a row,
/// which its restart lifted). The settling start and two more start; at each the pair's failure
/// state is what it was before the settlement, and the late run reads as interrupted: the
/// snapshot shows it as the pair's latest run, with the pair's failures in a row as before, and
/// the pair's outcome is missing (its latest completed run failed).
#[test]
fn unset2_n_the_starts_after_a_late_settlement_keep_the_failure_state() {
    // Nothing here lifts UNSET-1's hold, so A stays the mapped fixture's simulation.
    let fixture = Unset2Backend::Fixture;
    for held_interruption in [false, true] {
        let case = if held_interruption {
            "after an interruption"
        } else {
            "the pair's first run"
        };
        let root = TempDir::new().expect("tempdir");
        let (host, state, first, _, _, clock) =
            admitted_mapped_run_fixture_configured(&root, UNSET2_OUTCOME_KEY, unset2_scheduled);
        let instance = first.lease_token().instance_id();
        let (host, cut) = if held_interruption {
            // The first run is cut by a stop before any record of its own; the next start
            // settles it as interrupted and lifts that first interruption.
            host.close().expect("close with the first run open");
            unset2_tick(&clock);
            let host = unset2_mapped_start(&root, &clock, &state, instance, None, fixture);
            assert_eq!(
                unset2_decision_records(
                    &host,
                    EventType::PolicyDispatchCompleted,
                    first.decision_id()
                ),
                1,
                "{case}: the first run is settled"
            );
            let at = first
                .admission()
                .activity
                .next_eligible_unix_ms
                .max(POLICY_NOW_UNIX_MS + MAPPED_RUN_MIN_ADVANCE_MS);
            advance_manual_clock_to(&clock, at);
            let (cut, _) = admit_mapped_run_at(&host, UNSET2_OUTCOME_KEY, at, 42_201);
            (host, cut)
        } else {
            (host, first)
        };
        let cut_run = cut.run_id();
        let cut_admitted = cut.admission().activity.admitted_at_unix_ms;
        let (alias, task) = (
            cut.instance_alias().to_owned(),
            cut.catalog_task_id().to_owned(),
        );
        unset2_cut_after_its_own_release(host, &cut);
        let host = unset2_mapped_hold(&root, &clock, &state, instance, cut_run, fixture);
        let before = unset2_pair_record(&host, &alias, &task);
        assert_eq!(
            before.failure_streak.is_some(),
            held_interruption,
            "{case}: the held failure: {before:#?}"
        );
        host.close().expect("close with A held");

        for start in ["the settling start", "the next start", "the start after it"] {
            let at = format!("{case}, {start}");
            let now = unset2_tick(&clock);
            let host = unset2_mapped_start(&root, &clock, &state, instance, None, fixture);
            assert_settled_once_and_closed(&host, cut_run, cut.decision_id());
            let after = unset2_pair_record(&host, &alias, &task);
            assert_failure_state_kept(&before, &after, &at);
            let snapshot = unset2_snapshot(&host, &alias, &task, now);
            assert_eq!(
                snapshot.last_dispatched_unix_ms,
                Some(cut_admitted),
                "{at}: the late run is the pair's latest"
            );
            assert_eq!(
                snapshot.terminal_state,
                Some(actingcommand_policy::TaskTerminalState::Failed),
                "{at}: it reads as interrupted"
            );
            assert_eq!(
                snapshot.failure_streak,
                Some(u16::from(held_interruption)),
                "{at}: the pair's failures in a row are as before"
            );
            let outcomes = host
                .policy_outcome_key_snapshot_for_test()
                .expect("the outcome-key snapshot");
            let latest = outcomes
                .completed_runs
                .get(&(task.clone(), alias.clone()))
                .unwrap_or_else(|| panic!("{at}: the pair's latest completed run"));
            assert_eq!(latest.run_id, cut_run, "{at}");
            assert!(
                matches!(
                    latest.execution_outcome,
                    PolicyExecutionOutcome::Failed { .. }
                ),
                "{at}: the pair's outcome is missing"
            );
            assert!(host.fatal_error().expect("runtime health").is_none());
            host.close().expect("close host");
        }
    }
}

/// UNSET-2 fix 2 (UNSET2-5; the second review's §4): the dispatch journal's pruning keeps a late
/// completion, admitted before the journal's bound of later runs of its pair and completed after
/// all of them, while the value records' position lies before its completion, and drops it once
/// that position is at or after it, or without a position (a full replay); the later runs stay.
#[test]
fn unset2_the_journal_keeps_a_late_completion_until_a_value_record_follows_it() {
    let late_completed: u64 = 1_000;
    for (recorded_through, kept) in [
        (Some(late_completed - 1), true),
        (Some(late_completed), false),
        (Some(late_completed + 1), false),
        (None, false),
    ] {
        let (late, size) = crate::policy_host::journal_keeps_a_late_completion_for_test(
            late_completed,
            recorded_through,
        );
        assert_eq!(late, kept, "recorded through {recorded_through:?}");
        assert_eq!(
            size,
            actingcommand_policy::REMEDY_JOURNAL_MAX_RUNS + usize::from(kept),
            "recorded through {recorded_through:?}: the later runs stay"
        );
    }
}

fn two_instance_pending_policy_host(root: &TempDir) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("two-instance policy host");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("two-instance catalog activation");
    host
}

fn evaluate_pending_policy(host: &RuntimeHost, trigger: PolicyTrigger) -> PolicyCycle {
    host.evaluate_policy_cycle_with_test_inputs(
        &pending_policy_facts(),
        &pending_policy_resources(),
        EvaluationTime {
            unix_ms: POLICY_NOW_UNIX_MS,
            monotonic_ms: POLICY_NOW_UNIX_MS,
        },
        7,
        trigger,
    )
    .expect("two-instance evaluation")
}

fn policy_rejections(host: &RuntimeHost) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyDispatchRejected),
        ..EventQuery::default()
    })
    .expect("policy rejections")
}

#[test]
fn a_held_instance_defers_its_candidates_while_another_instance_dispatches() {
    // W-1: a held instance's candidates are deferred `dispatch_instance_held` with no wake and
    // nothing written; another instance dispatches in the same cycle; once the key returns, the
    // held instance dispatches.
    use actingcommand_policy::SchedulingDecisionState;

    let root = TempDir::new().expect("tempdir");
    let host = two_instance_pending_policy_host(&root);
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS);

    let cycle = evaluate_pending_policy(&host, PolicyTrigger::FactsChanged);
    assert_eq!(
        cycle
            .pending_dispatch_intents
            .iter()
            .map(|intent| intent.instance_id.as_str())
            .collect::<Vec<_>>(),
        vec![POLICY_INSTANCE_ALIAS_B],
        "B dispatches in the cycle that defers A"
    );
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    let deferred = evaluation
        .decisions
        .iter()
        .find(|decision| decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS))
        .expect("A's decision");
    assert_eq!(deferred.state, SchedulingDecisionState::Deferred);
    let held = deferred
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "dispatch_instance_held")
        .expect("held deferral reason");
    assert_eq!(held.detail, "holder_kind=client_lease count=0");
    assert!(
        host.query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyDispatchIntent),
            ..EventQuery::default()
        })
        .expect("dispatch intents")
        .is_empty(),
        "nothing is written for a deferred candidate"
    );

    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    let cycle = evaluate_pending_policy(&host, PolicyTrigger::Recovery);
    assert!(
        cycle
            .pending_dispatch_intents
            .iter()
            .any(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS),
        "A dispatches once its key returned: {:#?}",
        cycle.pending_dispatch_intents
    );
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn an_admission_that_meets_a_holder_is_rejected_held_at_info() {
    // W-1 (admission race): the admission reads the holder under the scheduler lock; only then
    // does it write the intent and an Info `dispatch_rejected` with `dispatch_instance_held`.
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::clone(&state),
        )),
    )
    .expect("held admission host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS);

    let failure = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("a held instance refuses the dispatch");
    assert_eq!(failure.code(), "dispatch_instance_held");
    assert!(!failure.is_fatal());
    let rejections = policy_rejections(&host);
    let [rejected] = rejections.as_slice() else {
        panic!("one rejection: {rejections:#?}");
    };
    assert_eq!(rejected.severity(), EventSeverity::Info);
    let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = rejected.payload() else {
        panic!("rejection payload");
    };
    let rejection = payload.rejection().expect("rejection facts");
    assert_eq!(rejection.outcome.code().as_str(), "dispatch_instance_held");
    // Workflow #378 A4 (part B L3, L4): the resolved instance's identifier, the holder's kind
    // and the number of eligible waiting claims (none waits).
    assert_eq!(
        rejection.outcome.value("instance_id"),
        Some(&serde_json::to_value(instance).expect("instance id"))
    );
    assert_eq!(
        rejection.outcome.value("holder_kind"),
        Some(&serde_json::json!("client_lease"))
    );
    assert_eq!(
        rejection.outcome.value("count"),
        Some(&serde_json::json!(0))
    );
    // Review M1: a wait is not a failure. Its key tokens are recorded at Info; no Error
    // `runtime.failed` follows the rejection (§5.10: no error point).
    let failures = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("runtime failures");
    assert!(
        failures
            .iter()
            .all(|event| event.severity() == EventSeverity::Info),
        "{failures:#?}"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn guard_contention_without_a_holder_is_rejected_contended_at_info() {
    // W-1 (review L-6): an observe or probe holding the admission guard is not a hold. The
    // admission retries `try_lock` for about 100 ms, then ends the intent it wrote with an
    // Info `dispatch_instance_contended` carrying `next_eligible_unix_ms` about 1 s ahead; no
    // `dispatch_instance_held` is written.
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = Arc::new(FakeState::default());
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::clone(&state),
        )),
    )
    .expect("contended admission host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .instance_admission_for_test(POLICY_INSTANCE_ALIAS)
        .expect("admission guard");
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let observer = thread::spawn(move || {
        let _guard = admission.lock().expect("hold the admission guard");
        held_tx.send(()).expect("report the held guard");
        release_rx.recv().expect("release signal");
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("guard held");

    let failure = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("a contended guard ends the intent");
    release_tx.send(()).expect("let the guard go");
    observer.join().expect("guard holder");
    assert_eq!(failure.code(), "dispatch_instance_contended");
    assert!(!failure.is_fatal());
    let rejections = policy_rejections(&host);
    let [rejected] = rejections.as_slice() else {
        panic!("one rejection: {rejections:#?}");
    };
    assert_eq!(rejected.severity(), EventSeverity::Info);
    let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = rejected.payload() else {
        panic!("rejection payload");
    };
    let rejection = payload.rejection().expect("rejection facts");
    assert_eq!(
        rejection.outcome.code().as_str(),
        "dispatch_instance_contended"
    );
    assert_eq!(
        rejection.next_eligible_unix_ms,
        Some(intent.prerequisites.evaluated_at_unix_ms + 1_000)
    );
    // Workflow #378 A4 (part B L3, L4): the resolved instance's identifier and the time the
    // candidate may run again.
    assert_eq!(
        rejection.outcome.value("instance_id"),
        Some(&serde_json::to_value(instance).expect("instance id"))
    );
    assert_eq!(
        rejection.outcome.value("next_eligible_unix_ms"),
        Some(&serde_json::json!(
            intent.prerequisites.evaluated_at_unix_ms + 1_000
        ))
    );
    assert!(
        host.query_persisted_events_for_test(EventQuery::default())
            .expect("ledger")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) => {
                    payload
                        .rejection()
                        .map(|rejection| rejection.outcome.code().as_str().to_owned())
                }
                _ => None,
            })
            .all(|code| code != "dispatch_instance_held"),
        "contention is not a hold"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// Test plan v2.2 §4 #15: a child-process entry, not a test; its parent runs it with
// `--ignored --exact`.
#[test]
#[ignore]
fn takeover_cooldown_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_TAKEOVER_COOLDOWN_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let instance_id: InstanceId =
        serde_json::from_slice(&fs::read(root.join("instance.json")).expect("instance bytes"))
            .expect("instance identifier");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"takeover-cooldown-process-salt")
            .with_procedure_manifest(procedure_manifest()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("cooldown child host");
    let mut client = TestClient::connect(&host);
    let _ = client.acquire(POLICY_INSTANCE_ALIAS);
    fs::write(root.join("lease-held.marker"), b"held").expect("held marker");
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn a_takeover_cooldown_defers_candidates_with_a_wake_at_its_end() {
    // Workflow #369 W-1 (review L3): a candidate on an instance in takeover cooldown is
    // deferred `dispatch_instance_cooldown`, with a wake at the cooldown's end; nothing is
    // written for it.
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let marker = root.path().join("lease-held.marker");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--ignored",
            "--exact",
            "tests::policy_admission::takeover_cooldown_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_TAKEOVER_COOLDOWN_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cooldown child");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.is_file() {
        assert!(Instant::now() < deadline, "held-lease marker timeout");
        assert!(
            child.try_wait().expect("poll cooldown child").is_none(),
            "the child exited before it held the lease"
        );
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("kill cooldown child");
    child.wait().expect("wait cooldown child");

    let host = RuntimeHost::start(
        config(&root).with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 600_000,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        }),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("takeover host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluation under a takeover cooldown");
    let evaluation = cycle.evaluation.expect("evaluation");
    assert!(evaluation.dispatch_intents.is_empty(), "{evaluation:#?}");
    let reason = evaluation
        .decisions
        .iter()
        .flat_map(|decision| decision.reasons.iter())
        .find(|reason| reason.outcome.code().as_str() == "dispatch_instance_cooldown")
        .unwrap_or_else(|| panic!("cooldown deferral: {evaluation:#?}"));
    let wake = reason
        .detail
        .strip_prefix("next_eligible_unix_ms=")
        .expect("one key token")
        .parse::<u64>()
        .expect("wake time");
    assert!(
        wake > POLICY_NOW_UNIX_MS && wake <= POLICY_NOW_UNIX_MS + 600_000,
        "{wake}"
    );
    assert!(
        evaluation
            .next_wake_unix_ms
            .is_some_and(|next_wake| next_wake <= wake)
    );
    assert!(
        policy_rejections(&host).is_empty(),
        "nothing is written for a deferred candidate"
    );
    host.close().expect("close host");
}
