// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-3 (#447, register §1.332) on a real policy host: what `PolicyHost::idle_runs_counted`
//! answers as the policy's own dispatches of one neutral instance go through their lifecycle across
//! evaluated cycles, in the defect's order. The harness is the mapped chain of `mapped_outcomes`
//! (`mapped_policy_sources`): `fixture.observe` runs a package that publishes an outcome, and
//! `fixture.followup`'s trigger reads that outcome, so the second task is due once the first has
//! completed. The evaluations run at fixed times on the manual Runtime clock; the plan and the
//! answer are read as the idle check reads them, under one policy guard
//! (`RuntimeHost::idle_plan_and_runs_counted_for_test`).
//!
//! What the harness does not reach apart: the live admission appends a dispatch's intent and its
//! admission under one guard, so the intent is read together with its admission; one settlement
//! records the run's execution and its completion; and a dispatch a start could not settle
//! (`quarantine_dispatch`) needs a start whose settlement refuses the run.

use super::*;

/// The outcome the first task's package publishes and the second task's trigger reads.
const OUTCOME_KEY: &str = "first-run-ended";

/// The latest plan of the fixture instance and whether it counts the end of every run the policy
/// dispatched there.
fn read(host: &RuntimeHost) -> (Option<crate::idle_action::IdlePlan>, bool) {
    host.idle_plan_and_runs_counted_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the idle plan and its count")
}

/// When the latest plan was evaluated, if there is one.
fn evaluated_at(host: &RuntimeHost) -> Option<u64> {
    read(host).0.map(|plan| plan.evaluated_at_unix_ms)
}

/// One evaluated cycle at `unix_ms` (also the manual clock's time) on `facts`.
fn evaluate(
    host: &RuntimeHost,
    facts: &EvaluationFacts,
    unix_ms: u64,
    seed: u64,
    trigger: PolicyTrigger,
) -> PolicyCycle {
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            facts,
            &policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            trigger,
        )
        .unwrap_or_else(|error| panic!("the evaluation at {unix_ms}: {error}"));
    assert!(
        cycle.evaluation.is_some(),
        "the cycle at {unix_ms} is evaluated: {:?}",
        cycle.directive
    );
    cycle
}

/// The dispatch intent of `task_id` in `cycle`'s evaluation, with its reason chain.
fn intent_of(cycle: &PolicyCycle, task_id: &str) -> (DispatchIntent, DecisionReasonChain) {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| {
            panic!(
                "a dispatch of {task_id}: {:?}",
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
        .expect("the dispatch's reason chain")
        .clone();
    (intent, reasons)
}

#[test]
fn idle_runs_counted_follows_the_policys_dispatches() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_mapped_contained_task_package(OUTCOME_KEY, "designated_effect_completed");
    let package_path = root.path().join("first-task.zip");
    fs::write(&package_path, &package).expect("write the first task's package");
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(
                mapped_policy_facts(OUTCOME_KEY, false),
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
    host.activate_policy_catalog(&mapped_policy_sources(1, OUTCOME_KEY))
        .expect("activate the chained catalog");

    // Before any evaluated cycle: no plan, counted.
    assert_eq!(read(&host), (None, true), "before any evaluated cycle");

    // A cycle evaluated with nothing open: counted. It dispatches the first task.
    let first_cycle = evaluate(
        &host,
        &mapped_policy_facts(OUTCOME_KEY, false),
        POLICY_NOW_UNIX_MS,
        70,
        PolicyTrigger::FactsChanged,
    );
    assert_eq!(evaluated_at(&host), Some(POLICY_NOW_UNIX_MS));
    assert!(read(&host).1, "a cycle evaluated with nothing open");

    // That cycle's dispatch of the first task is recorded (its intent, with its admission): it was
    // opened after the plan's read.
    let (first_intent, first_reasons) = intent_of(&first_cycle, "fixture.observe");
    record_policy_approval(&host, &first_intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(
            &first_intent,
            &first_reasons,
            &policy_context(&host, &first_intent),
        )
        .expect("the first task's admission")
    else {
        panic!("the first task's run context")
    };
    assert!(
        !read(&host).1,
        "the first task's dispatch was opened after the plan's read"
    );

    // A cycle evaluated while that dispatch is admitted (the policy's state of a running dispatch
    // until its execution is recorded): it reads the dispatch open. `Recovery` evaluates within
    // the cadence's cooldown.
    let during_unix_ms = POLICY_NOW_UNIX_MS + 1;
    advance_manual_clock_to(&clock, during_unix_ms);
    evaluate(
        &host,
        &mapped_policy_facts(OUTCOME_KEY, false),
        during_unix_ms,
        71,
        PolicyTrigger::Recovery,
    );
    assert_eq!(evaluated_at(&host), Some(during_unix_ms));
    assert!(
        !read(&host).1,
        "the first task's dispatch was open as the cycle read its inputs"
    );

    // The run ends and its execution and completion are recorded; no new evaluation: the latest
    // plan still read the dispatch open.
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the first task's request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the first task's run");
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the first task's settlement");
    assert_eq!(evaluated_at(&host), Some(during_unix_ms));
    assert!(
        !read(&host).1,
        "the latest plan read the first task's dispatch open"
    );

    // A cycle evaluated after the completion: its inputs read nothing open, and its plan shows the
    // second task due.
    let after_unix_ms = context
        .admission()
        .activity
        .next_eligible_unix_ms
        .max(during_unix_ms + 1_000);
    advance_manual_clock_to(&clock, after_unix_ms);
    let sample_capacity = host.capacity_sampler_for_test().expect("capacity owner");
    sample_capacity().expect("a capacity sample for the second task's admission");
    let after = evaluate(
        &host,
        &mapped_policy_facts_at(OUTCOME_KEY, after_unix_ms, false),
        after_unix_ms,
        72,
        PolicyTrigger::FactsChanged,
    );
    let (plan, counted) = read(&host);
    assert!(counted, "a cycle evaluated after the completion");
    let plan = plan.expect("the plan after the completion");
    assert_eq!(plan.evaluated_at_unix_ms, after_unix_ms, "{plan:?}");
    assert!(plan.due_now, "the second task is due: {plan:?}");

    // Once the second task's dispatch is recorded: not counted again.
    let (second_intent, second_reasons) = intent_of(&after, "fixture.followup");
    let admitted = host
        .admit_policy_dispatch(
            &second_intent,
            &second_reasons,
            &policy_context(&host, &second_intent),
        )
        .expect("the second task's admission");
    assert!(
        matches!(admitted, PolicyDispatchAdmission::Granted { .. }),
        "{admitted:?}"
    );
    assert!(
        !read(&host).1,
        "the second task's dispatch was opened after the plan's read"
    );
    host.close().expect("close the host");
}
