// SPDX-License-Identifier: AGPL-3.0-only

fn admitted_mapped_run_fixture(
    root: &TempDir,
    outcome_key: &str,
) -> (
    RuntimeHost,
    Arc<FakeState>,
    Box<PolicyRunContext>,
    ContainedTaskRequest,
) {
    let (host, state, context, request, _, _) =
        admitted_mapped_run_fixture_with_policy_time(root, outcome_key);
    (host, state, context, request)
}

fn admitted_mapped_run_fixture_with_policy_time(
    root: &TempDir,
    outcome_key: &str,
) -> (
    RuntimeHost,
    Arc<FakeState>,
    Box<PolicyRunContext>,
    ContainedTaskRequest,
    u64,
    Arc<ManualRuntimeClock>,
) {
    let policy_unix_ms = POLICY_NOW_UNIX_MS;
    let clock = Arc::new(ManualRuntimeClock::new(policy_unix_ms, policy_unix_ms));
    let package = neutral_mapped_contained_task_package(outcome_key, "designated_effect_completed");
    let package_path = root.path().join(format!("{outcome_key}-mapped-task.zip"));
    fs::write(&package_path, &package).expect("write mapped package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(root)
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
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .fixture_simulation(),
        ),
    )
    .expect("mapped runtime host");
    host.activate_policy_catalog(&mapped_policy_sources(1, outcome_key))
        .expect("activate mapped catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts(outcome_key, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: policy_unix_ms,
                monotonic_ms: policy_unix_ms,
            },
            70,
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
        .expect("mapped policy admission")
    else {
        panic!("expected mapped policy context")
    };
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("mapped package request");
    (host, state, context, request, policy_unix_ms, clock)
}

#[derive(Debug, Clone, Copy)]
struct MappedPolicyTimeline {
    first_admitted_at_unix_ms: u64,
    first_interval_ms: u64,
    first_next_eligible_unix_ms: u64,
    minimum_second_policy_unix_ms: u64,
    second_policy_unix_ms: u64,
    clock_delta_ms: u64,
}

struct MappedPolicyTimelineFixture {
    timeline: MappedPolicyTimeline,
    second_context: Box<PolicyRunContext>,
    second_directive: PolicyRecomputeDirective,
}

fn prepare_second_mapped_policy_time(
    first_context: &PolicyRunContext,
    clock: &ManualRuntimeClock,
) -> MappedPolicyTimeline {
    let first_activity = &first_context.admission().activity;
    assert_eq!(first_activity.admitted_at_unix_ms, POLICY_NOW_UNIX_MS);
    assert!((60_000..=300_000).contains(&first_activity.interval_ms));
    assert_eq!(
        first_activity.next_eligible_unix_ms,
        first_activity
            .admitted_at_unix_ms
            .checked_add(first_activity.interval_ms)
            .expect("mapped activity cadence overflow")
    );
    let minimum_second_policy_unix_ms = first_activity
        .admitted_at_unix_ms
        .checked_add(MAPPED_RUN_MIN_ADVANCE_MS)
        .expect("mapped policy time overflow");
    let second_policy_unix_ms =
        minimum_second_policy_unix_ms.max(first_activity.next_eligible_unix_ms);
    assert!(second_policy_unix_ms >= minimum_second_policy_unix_ms);
    assert!(second_policy_unix_ms >= first_activity.next_eligible_unix_ms);
    let clock_delta_ms = advance_manual_clock_to(clock, second_policy_unix_ms);
    MappedPolicyTimeline {
        first_admitted_at_unix_ms: first_activity.admitted_at_unix_ms,
        first_interval_ms: first_activity.interval_ms,
        first_next_eligible_unix_ms: first_activity.next_eligible_unix_ms,
        minimum_second_policy_unix_ms,
        second_policy_unix_ms,
        clock_delta_ms,
    }
}

fn advance_manual_clock_to(clock: &ManualRuntimeClock, target_unix_ms: u64) -> u64 {
    let before = clock.sample().expect("sample mapped runtime clock");
    let delta_ms = target_unix_ms
        .checked_sub(before.unix_ms)
        .expect("mapped runtime clock cannot move backwards");
    clock.advance(delta_ms);
    let after = clock.sample().expect("sample advanced runtime clock");
    assert_eq!(after.unix_ms, target_unix_ms);
    assert_eq!(
        after.monotonic_ms,
        before
            .monotonic_ms
            .checked_add(delta_ms)
            .expect("mapped monotonic clock overflow")
    );
    eprintln!(
        "workflow110_clock_advance before_unix={} before_monotonic={} target_unix={} delta={} after_unix={} after_monotonic={}",
        before.unix_ms,
        before.monotonic_ms,
        target_unix_ms,
        delta_ms,
        after.unix_ms,
        after.monotonic_ms,
    );
    delta_ms
}

fn admit_second_mapped_policy_run(
    case: &str,
    host: &RuntimeHost,
    first_context: &PolicyRunContext,
    clock: &ManualRuntimeClock,
    outcome_key: &str,
    seed: u64,
) -> MappedPolicyTimelineFixture {
    let timeline = prepare_second_mapped_policy_time(first_context, clock);
    let (second_context, second_directive) =
        admit_mapped_run_at(host, outcome_key, timeline.second_policy_unix_ms, seed);
    assert_eq!(
        second_context.admission().activity.admitted_at_unix_ms,
        timeline.second_policy_unix_ms
    );
    assert_eq!(second_directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        second_directive.reason,
        PolicyRecomputeReason::Reconciliation
    );
    eprintln!(
        "workflow110_timeline case={case} first_admitted_at={} first_interval={} first_next_eligible={} minimum_second={} selected_second={} clock_delta={} second_admitted_at={} second_directive_kind={:?} second_directive_reason={:?} second_eligible_at={}",
        timeline.first_admitted_at_unix_ms,
        timeline.first_interval_ms,
        timeline.first_next_eligible_unix_ms,
        timeline.minimum_second_policy_unix_ms,
        timeline.second_policy_unix_ms,
        timeline.clock_delta_ms,
        second_context.admission().activity.admitted_at_unix_ms,
        second_directive.kind,
        second_directive.reason,
        second_directive.eligible_at_unix_ms,
    );
    MappedPolicyTimelineFixture {
        timeline,
        second_context,
        second_directive,
    }
}

fn admitted_physical_run_fixture(
    root: &TempDir,
) -> (
    RuntimeHost,
    Arc<FakeState>,
    Box<PolicyRunContext>,
    ContainedTaskRequest,
    Arc<std::sync::Mutex<ResolvedExecutionInstance>>,
) {
    let package = neutral_contained_task_package();
    let package_path = root.path().join("physical-scheduled-task.zip");
    fs::write(&package_path, &package).expect("write physical package");
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let resolved = Arc::new(std::sync::Mutex::new(ResolvedExecutionInstance::new(
        instance_id(),
        "127.0.0.1:16384",
    )));
    let registered_instance = resolved
        .lock()
        .expect("physical resolved instance poisoned")
        .instance_id();
    let host = RuntimeHost::start(
        config(root).with_procedure_manifest(procedure_manifest_with_primary(
            &package,
            vec!["after_observation".to_owned()],
        )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_instance,
                Arc::clone(&state),
            )
            .with_resolved_override(Arc::clone(&resolved)),
        ),
    )
    .expect("physical runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate physical policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let request =
        ContainedTaskRequest::new(package_path.to_string_lossy().into_owned(), package_sha256)
            .expect("physical package request");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(
            &intent,
            &reasons,
            &policy_context(&host, &intent),
            &request,
        )
        .expect("physical policy admission")
    else {
        panic!("expected physical policy context")
    };
    (host, state, context, request, resolved)
}

fn admit_mapped_run_at(
    host: &RuntimeHost,
    outcome_key: &str,
    unix_ms: u64,
    seed: u64,
) -> (Box<PolicyRunContext>, PolicyRecomputeDirective) {
    host.shared
        .performance
        .lock()
        .unwrap()
        .sample_and_record_capacity(&host.shared.ledger, &host.shared.events)
        .expect("capacity sample for mapped admission time");
    let facts = mapped_policy_facts_at(outcome_key, unix_ms, true);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            PolicyTrigger::Reconciliation,
        )
        .expect("mapped policy evaluation");
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
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(host, &intent))
        .expect("mapped policy admission")
    else {
        panic!("expected mapped run context")
    };
    (context, cycle.directive)
}

fn evaluate_mapped_policy_after_outcome(
    case: &str,
    fixture: &MappedPolicyTimelineFixture,
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    outcome_key: &str,
    deferred_seed: u64,
    evaluation_seed: u64,
) -> PolicyCycle {
    let before_deferred = clock.sample().expect("sample post-outcome policy clock");
    assert_eq!(
        before_deferred.unix_ms,
        fixture.timeline.second_policy_unix_ms
    );
    let deferred = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts_at(outcome_key, before_deferred.unix_ms, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: before_deferred.unix_ms,
                monotonic_ms: before_deferred.monotonic_ms,
            },
            deferred_seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate mapped cooldown boundary");
    assert_eq!(deferred.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(deferred.directive.reason, PolicyRecomputeReason::Cooldown);
    assert!(deferred.evaluation.is_none());

    let next_policy_unix_ms = deferred.directive.eligible_at_unix_ms;
    let cooldown_delta_ms = advance_manual_clock_to(clock, next_policy_unix_ms);
    let after_advance = clock.sample().expect("sample executable policy clock");
    let evaluated = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts_at(outcome_key, next_policy_unix_ms, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: next_policy_unix_ms,
                monotonic_ms: after_advance.monotonic_ms,
            },
            evaluation_seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate mapped policy after cooldown");
    assert_eq!(evaluated.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(evaluated.directive.reason, PolicyRecomputeReason::Event);
    assert!(evaluated.evaluation.is_some());
    eprintln!(
        "workflow110_post_outcome case={case} second_kind={:?} second_reason={:?} second_eligible_at={} deferred_at={} deferred_kind={:?} deferred_reason={:?} deferred_eligible_at={} cooldown_delta={} evaluated_at={} evaluated_kind={:?} evaluated_reason={:?}",
        fixture.second_directive.kind,
        fixture.second_directive.reason,
        fixture.second_directive.eligible_at_unix_ms,
        before_deferred.unix_ms,
        deferred.directive.kind,
        deferred.directive.reason,
        deferred.directive.eligible_at_unix_ms,
        cooldown_delta_ms,
        after_advance.unix_ms,
        evaluated.directive.kind,
        evaluated.directive.reason,
    );
    let activity_eligible = fixture
        .second_context
        .admission()
        .activity
        .next_eligible_unix_ms;
    assert!(activity_eligible > next_policy_unix_ms);
    assert!(
        evaluated.pending_dispatch_intents.is_empty(),
        "driver cooldown cannot bypass the real activity interval"
    );
    advance_manual_clock_to(clock, activity_eligible);
    let at = clock.sample().expect("sample activity-eligible clock");
    let available = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_policy_facts_at(outcome_key, at.unix_ms, false),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at.unix_ms,
                monotonic_ms: at.monotonic_ms,
            },
            evaluation_seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the exact recorded activity boundary");
    assert_eq!(available.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(available.directive.reason, PolicyRecomputeReason::Event);
    available
}

fn mapped_policy_facts_at(outcome_key: &str, unix_ms: u64, stop_followup: bool) -> EvaluationFacts {
    let mut facts = mapped_policy_facts(outcome_key, false);
    for fact in &mut facts.facts {
        fact.observed_at_unix_ms = unix_ms;
        fact.expires_at_unix_ms = Some(unix_ms + 900_000);
    }
    let followup_stop = facts
        .facts
        .iter_mut()
        .find(|fact| fact.fact_key == "fixture.followup.stop")
        .expect("followup stop fact");
    followup_stop.value = FactValue::Boolean(stop_followup);
    facts.fact_snapshot_id = format!("snapshot:mapped:{outcome_key}:next-run");
    facts
}
