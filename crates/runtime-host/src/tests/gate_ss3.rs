// SPDX-License-Identifier: AGPL-3.0-only

// SS-3 (Alice's ruling 100 on Workflow #386, comment 6490879952: dispatch follows live account
// state and "run task X N times" is gone; rd-1029 design-sch-state §1.1, §4, §5 SS-3; register
// §1.212): run-count quotas become optional. In an `actingcommand.scheduling.v2` catalog
// `loop_budget`, each of its three items and each of an activity profile's three run-count bounds
// may be left out, and one that is not written is no dimension: admission reserves nothing for
// it, the retry round does not judge it, the admission receipt carries none of its members and a
// window's cumulative runtime is never judged without a runtime bound. A written item applies
// exactly as before.
//
// A neutral fixture: the example catalog `catalog-a`, as V1 or turned into V2, its activity
// interval set to 0 so that admissions are spaced by nothing but the bounds under test, and the
// Runtime's own control state driven with a fixed clock (no sleep, no wall clock).

use crate::RuntimeHostError;
use crate::policy_control::{PolicyControlState, PolicyExecutionInput, PolicyExecutionTiming};
use crate::policy_host::{BlackoutContext, BlackoutGate};
use actingcommand_contract::{
    ContainedTaskRequest, PerformanceContext, PolicyAdmissionRecord, PolicyBudgetDenial,
    PolicyBudgetDimension, PolicyBudgetReceipt, PolicyExecutionOutcome,
};
use actingcommand_policy::{
    BlackoutEntry, BlackoutWindow, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    DispatchIntent, DispatchPrerequisites, EvaluationTime, InstanceSnapshot, LoadProfile,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, compile_catalog,
};
use actingcommand_runtime_config::scheduling::QuietHorizon;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// 2023-11-14 12:00 UTC, inside the fixture's activity window (08:00 to 22:00 UTC).
const NOW: u64 = 1_699_963_200_000;
const INSTANCE: &str = "fixture-instance-a";
const PROFILE: &str = "fixture-activity-a";
const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;

fn document(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("fixture JSON")
}

/// `catalog-a` declared as `version`, with `task` applied to its task and `profile` to the
/// instance's activity profile.
fn catalog(
    version: &str,
    task: impl FnOnce(&mut Value),
    profile: impl FnOnce(&mut Value),
) -> CompiledCatalog {
    let mut tasks = document(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let mut pools = document(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/pools.json"
    ));
    let mut activity = document(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/activity.json"
    ));
    let mut timeline = document(include_bytes!(
        "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
    ));
    for declared in [&mut tasks, &mut pools, &mut activity, &mut timeline] {
        declared["schema_version"] = json!(version);
    }
    if version == SCHEDULING_SCHEMA_VERSION_V2 {
        for event in timeline["events"].as_array_mut().expect("timeline events") {
            event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
        }
    }
    for each in activity["profiles"]
        .as_array_mut()
        .expect("activity profiles")
    {
        each["minimum_interval_ms"] = json!(0);
        each["maximum_interval_ms"] = json!(0);
    }
    task(&mut tasks["tasks"][0]);
    profile(&mut activity["profiles"][0]);
    let source = |uri: &str, value: &Value| {
        CatalogDocumentSource::new(uri, serde_json::to_vec(value).expect("fixture bytes"))
    };
    compile_catalog(&CatalogSources {
        tasks: source("memory://ss3/tasks.json", &tasks),
        pools: source("memory://ss3/pools.json", &pools),
        activity: source("memory://ss3/activity.json", &activity),
        timeline: source("memory://ss3/timeline.json", &timeline),
        selection: None,
    })
    .expect("SS-3 fixture catalog")
}

fn remove(value: &mut Value, members: &[&str]) {
    let object = value.as_object_mut().expect("fixture object");
    for member in members {
        object.remove(*member);
    }
}

/// The instance's activity profile without any of its three run-count bounds.
fn unbounded_profile(profile: &mut Value) {
    remove(
        profile,
        &["daily_budget", "max_window_iterations", "session_max_ms"],
    );
}

fn intent(catalog: &CompiledCatalog, run: u64) -> DispatchIntent {
    let task = &catalog.catalog().tasks.tasks[0];
    DispatchIntent {
        decision_id: format!("decision:ss3-{run}"),
        task_id: task.id.clone(),
        instance_id: INSTANCE.to_owned(),
        operation_id: task.entrypoint.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        procedure_binding_digest: Some(format!("sha256:{}", "b".repeat(64))),
        catalog_hash: catalog.catalog_hash().to_owned(),
        catalog_version: catalog.catalog().tasks.catalog.catalog_version,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:ss3".to_owned(),
        approval_refs: catalog.catalog().tasks.catalog.approval_refs.clone(),
        reason_chain_id: format!("reason:ss3-{run}"),
        expected_duration_ms: task.expected_duration_ms,
        yield_points: task.yield_points.clone(),
        load_profile: LoadProfile::Light,
        prerequisites: DispatchPrerequisites {
            fencing_required: true,
            evaluated_at_unix_ms: NOW,
            facts_fresh_until_unix_ms: None,
            activity_profile_id: PROFILE.to_owned(),
            daily_limit: task.daily_limit(),
            window_iteration_limit: task.window_iteration_limit(),
            max_runtime_ms: task.max_runtime_ms(),
            urgency_milli: 0,
        },
    }
}

/// Admits run `run` at `at`; when admitted, settles it as a success of `runtime_ms` and returns
/// its admission and its execution record's outcome.
fn admit_and_succeed(
    state: &mut PolicyControlState,
    catalog: &CompiledCatalog,
    run: u64,
    at: u64,
    runtime_ms: u64,
) -> Result<(PolicyAdmissionRecord, PolicyExecutionOutcome), RuntimeHostError> {
    let intent = intent(catalog, run);
    let admission = state.preview_admission(catalog, &intent, at)?;
    state
        .commit_admission(catalog, &intent, &admission, false)
        .expect("commit the admission");
    let observed_at_unix_ms = at + runtime_ms;
    let data = state
        .preview_execution(
            catalog,
            &intent,
            &admission,
            PolicyExecutionTiming {
                observed_at_unix_ms,
                runtime_ms,
            },
            &PolicyExecutionInput::Succeeded,
            &PerformanceContext::unavailable(observed_at_unix_ms),
        )
        .expect("settle the run");
    state
        .commit_execution(catalog, &intent, &admission, &data)
        .expect("commit the settlement");
    Ok((admission, data.outcome))
}

/// The members the receipt serializes, by name.
fn receipt_members(receipt: &PolicyBudgetReceipt) -> Vec<String> {
    serde_json::to_value(receipt)
        .expect("receipt JSON")
        .as_object()
        .expect("receipt object")
        .keys()
        .cloned()
        .collect()
}

const TASK_COUNT_MEMBERS: [&str; 4] = [
    "task_daily_used",
    "task_daily_limit",
    "task_window_used",
    "task_window_limit",
];

// Test 1: without `loop_budget`, 50 consecutive runs of the pair in one activity window and day
// are all admitted; none meets a `TaskDaily` or `TaskWindow` refusal. The activity profile keeps
// bounds that 50 runs do not reach, so only the task's dimensions are under test.
#[test]
fn ss3_t1_without_loop_budget_fifty_runs_meet_no_task_count_refusal() {
    let catalog = catalog(
        SCHEDULING_SCHEMA_VERSION_V2,
        |task| remove(task, &["loop_budget"]),
        |profile| {
            profile["daily_budget"] = json!(1_000);
            profile["max_window_iterations"] = json!(1_000);
            profile["session_max_ms"] = json!(DAY_MS);
        },
    );
    assert_eq!(catalog.catalog().tasks.tasks[0].loop_budget, None);
    let mut state = PolicyControlState::default();
    for run in 0..50_u64 {
        let at = NOW + run * MINUTE_MS;
        match admit_and_succeed(&mut state, &catalog, run, at, 1_000) {
            Ok((admission, _)) => {
                let members = receipt_members(&admission.budget);
                for member in TASK_COUNT_MEMBERS {
                    assert!(
                        !members.iter().any(|written| written == member),
                        "run {run}: the receipt carries no {member}"
                    );
                }
                assert_eq!(admission.budget.activity_daily_used, Some(run as u32 + 1));
            }
            Err(error) => {
                let dimension = error
                    .policy_rejection()
                    .budget
                    .map(|denial| denial.dimension);
                assert!(
                    !matches!(
                        dimension,
                        Some(PolicyBudgetDimension::TaskDaily | PolicyBudgetDimension::TaskWindow)
                    ),
                    "run {run} met a task count refusal: {dimension:?}"
                );
                panic!("run {run} was refused: {}", error.code());
            }
        }
    }
}

// Test 2: a written `daily_limit` still applies exactly as before. With `daily_limit` 2 the only
// item written, the third run of the day is refused `policy_budget_exhausted` on `TaskDaily`.
#[test]
fn ss3_t2_a_written_daily_limit_of_two_still_refuses_the_third_run() {
    let catalog = catalog(
        SCHEDULING_SCHEMA_VERSION_V2,
        |task| task["loop_budget"] = json!({"daily_limit": 2}),
        unbounded_profile,
    );
    let mut state = PolicyControlState::default();
    for run in 0..2_u64 {
        let (admission, _) =
            admit_and_succeed(&mut state, &catalog, run, NOW + run * MINUTE_MS, 1_000)
                .expect("the first two runs are admitted");
        assert_eq!(admission.budget.task_daily_used, Some(run as u32 + 1));
        assert_eq!(admission.budget.task_daily_limit, Some(2));
    }
    let error = admit_and_succeed(&mut state, &catalog, 2, NOW + 2 * MINUTE_MS, 1_000)
        .expect_err("the third run is refused");
    assert_eq!(error.code(), "policy_budget_exhausted");
    assert_eq!(
        error.policy_rejection().budget,
        Some(PolicyBudgetDenial {
            dimension: PolicyBudgetDimension::TaskDaily,
            used: 2,
            requested: 1,
            limit: 2,
        })
    );
}

// Test 3: without `max_runtime_ms` (and without the profile's `session_max_ms`), the window's
// cumulative runtime is never judged. The pair runs as it does in use, one run after another:
// each run is admitted when the one before it ends and lasts the longest a policy run may take,
// its per-run deadline (30 minutes). Nineteen such runs, 12:00 to 21:30 UTC, add up to 9.5 hours,
// far above what the example writes (300 000 ms for the task, 7 200 000 ms for the profile), and
// none is rewritten to `policy_runtime_budget_exceeded`. The design's "above 24 hours" cannot
// happen in one window: the example's activity window is 14 hours long, and a pair's runs never
// overlap.
//
// The per-run deadline still applies to the pair: the next run, which would start less than its
// deadline before a blackout, is held back `blackout_approaching` with the deadline as the time it
// needs; with the blackout a full deadline away it is not held back.
#[test]
fn ss3_t3_without_max_runtime_back_to_back_runs_record_no_runtime_budget_exceeded() {
    let catalog = catalog(
        SCHEDULING_SCHEMA_VERSION_V2,
        |task| remove(task, &["loop_budget"]),
        unbounded_profile,
    );
    let per_run_ms = ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS;
    let mut state = PolicyControlState::default();
    let mut at = NOW;
    for run in 0..19_u64 {
        let (admission, outcome) = admit_and_succeed(&mut state, &catalog, run, at, per_run_ms)
            .expect("every run is admitted");
        assert_eq!(admission.activity.admitted_at_unix_ms, at);
        assert_eq!(admission.budget.task_runtime_reserved_ms, None);
        assert_eq!(admission.budget.activity_runtime_reserved_ms, None);
        assert_eq!(
            outcome,
            PolicyExecutionOutcome::Succeeded {
                runtime_ms: per_run_ms
            },
            "run {run} is not rewritten to policy_runtime_budget_exceeded"
        );
        // The next run is admitted when this one ends.
        at += per_run_ms;
    }
    let cumulative_ms = at - NOW;
    assert_eq!(cumulative_ms, 19 * per_run_ms);
    assert!(
        cumulative_ms > 7_200_000,
        "the window holds {cumulative_ms} ms"
    );

    let next = intent(&catalog, 19);
    let deadline_ms = crate::policy_host::evaluation_deadline_ms(&next);
    assert_eq!(deadline_ms, per_run_ms);
    let instance = InstanceSnapshot {
        instance_id: INSTANCE.to_owned(),
        server_id: "fixture-server-a".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: Vec::new(),
        preferred_task_ids: Vec::new(),
    };
    let quiet_gate = |blackout_start_unix_ms: u64| {
        BlackoutContext {
            host: vec![BlackoutEntry {
                id: "fixture-blackout".to_owned(),
                window: BlackoutWindow::Once {
                    from_unix_ms: blackout_start_unix_ms,
                    until_unix_ms: blackout_start_unix_ms + MINUTE_MS,
                },
            }],
            instances: BTreeMap::new(),
            day_boundary_quiet: BTreeMap::new(),
            day_boundary_section: false,
            quiet_margin_ms: 0,
            hard_quiet_ms: 0,
            quiet_horizon: QuietHorizon::Deadline,
        }
        .quiet_gate(
            &catalog,
            &instance,
            &next,
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: 0,
            },
            deadline_ms,
        )
        .expect("the quiet gate")
    };
    match quiet_gate(at + deadline_ms - 1) {
        Some(BlackoutGate::Approaching {
            blackout,
            required_ms,
        }) => {
            assert_eq!(blackout.id, "fixture-blackout");
            assert_eq!(required_ms, deadline_ms);
        }
        other => panic!("the next run is held back by its deadline: {other:?}"),
    }
    assert_eq!(quiet_gate(at + deadline_ms), None);
}

// Test 4: the admission receipt carries no dimension that was not written. With nothing written
// it serializes as `{}`; with only the task's `daily_limit` and the profile's `session_max_ms`
// written it carries exactly those two dimensions. The ledger's receipt rule takes both, and
// still refuses a dimension that carries only one of its two members.
#[test]
fn ss3_t4_receipts_carry_no_dimension_that_was_not_written() {
    let none_written = catalog(
        SCHEDULING_SCHEMA_VERSION_V2,
        |task| remove(task, &["loop_budget"]),
        unbounded_profile,
    );
    let mut state = PolicyControlState::default();
    let (admission, _) =
        admit_and_succeed(&mut state, &none_written, 0, NOW, 1_000).expect("the run is admitted");
    assert_eq!(
        serde_json::to_string(&admission.budget).expect("receipt JSON"),
        "{}"
    );
    assert!(admission.budget.is_valid());

    let two_written = catalog(
        SCHEDULING_SCHEMA_VERSION_V2,
        |task| task["loop_budget"] = json!({"daily_limit": 5}),
        |profile| {
            remove(profile, &["daily_budget", "max_window_iterations"]);
            profile["session_max_ms"] = json!(DAY_MS);
        },
    );
    let mut state = PolicyControlState::default();
    let (admission, _) =
        admit_and_succeed(&mut state, &two_written, 0, NOW, 1_000).expect("the run is admitted");
    let mut members = receipt_members(&admission.budget);
    members.sort();
    assert_eq!(
        members,
        [
            "activity_runtime_limit_ms",
            "activity_runtime_reserved_ms",
            "task_daily_limit",
            "task_daily_used",
        ]
    );
    assert!(admission.budget.is_valid());

    let half = PolicyBudgetReceipt {
        task_daily_limit: None,
        ..admission.budget.clone()
    };
    assert!(!half.is_valid(), "a dimension with one member is refused");
}

// Test 6 (host part): a catalog that writes all three items and the three activity bounds, the
// V1 example as it is, gives the receipt it gave before, byte for byte: all twelve members, in
// their order, with the values of the first run.
#[test]
fn ss3_t6_a_catalog_that_writes_every_bound_gives_the_same_receipt_bytes() {
    let catalog = catalog(SCHEDULING_SCHEMA_VERSION, |_| {}, |_| {});
    let mut state = PolicyControlState::default();
    let (admission, _) =
        admit_and_succeed(&mut state, &catalog, 0, NOW, 1_000).expect("the run is admitted");
    assert_eq!(
        serde_json::to_string(&admission.budget).expect("receipt JSON"),
        concat!(
            r#"{"task_daily_used":1,"task_daily_limit":24,"task_window_used":1,"#,
            r#""task_window_limit":4,"task_runtime_reserved_ms":60000,"#,
            r#""task_runtime_limit_ms":300000,"activity_daily_used":1,"#,
            r#""activity_daily_limit":24,"activity_window_used":1,"activity_window_limit":4,"#,
            r#""activity_runtime_reserved_ms":60000,"activity_runtime_limit_ms":7200000}"#
        )
    );
}
