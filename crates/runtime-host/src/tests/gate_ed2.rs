// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice ED-2 (frozen model v1.0 `rd-1024/sched-design/model.md` §3.4, §6 row ED-2, and the
// coordinator's ruling of 10-24 14:1x after the freeze; Alice 10-22: 「每个任务执行所需的时间可以
// 动态的从账本读，近5次该任务包的平均时间加5分钟」; ruling 39 R6.2 「即便版本升级，也不清空历史最近
// 五次的平均耗时」, R6.3 「放在账本的实例的基础信息库那一组标签下，启动时随模拟器连接完成载入内存」):
// the duration of every successful scheduled run becomes a sample of its task package on its
// instance, in the instance store record `health.run_duration`, which only the Runtime writes;
// the evaluation reads the expected duration E (the mean of the latest `runs` samples, rounded
// up, plus `margin_ms`, at most the task's runtime ceiling) through the task states, admission
// compares it, the dispatch intent records it, and `ac_schedule` rows show it.
//
// A neutral fixture: the fixture instance; two tasks, `fixture.observe` and its twin, bound to one
// procedure whose binding names the neutral package's location (so they share its samples);
// a runtime ceiling of ten hours, so E is never capped here; a manual Runtime clock; the package
// admission after start held. The Runtime maps a task to its package's samples once a scheduled
// run's preparation admitted the package since the start: a real run, or the test's stand-in
// for one (`learn_package_for_test`). Every expected value is the model's requirement.

use super::*;
use crate::pack_admission::PackAdmissionHold;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, PolicyConditionSettlement,
    RuntimeConfigManifest, RuntimeScheduleExplanation,
};
use actingcommand_policy::PolicyEvaluation;
use serde_json::{Value, json};

/// The neutral package's `package_id` (every neutral fixture package declares it).
const PACKAGE_ID: &str = "neutral.semantic.task";
const OBSERVE: &str = "fixture.observe";
const TWIN: &str = "fixture.observe-twin";
const PROCEDURE: &str = "procedure.observe";
/// The fixture task's declared duration D.
const DECLARED_MS: u64 = 60_000;
/// The shipped margin and number of runs.
const MARGIN_MS: u64 = 300_000;
const RUNS: usize = 5;
/// Ten hours: no E of these tests reaches it.
const MAX_RUNTIME_MS: u64 = 36_000_000;
const HOUR_MS: u64 = 3_600_000;
const ROW_DROPPED: &str = "run_duration_row_dropped";
const KEY_RESERVED: &str = "run_duration_key_reserved";
const STALE: &str = "policy_facts_stale";
const KEY: &str = "health.run_duration";

/// The region-click task of the neutral package, which succeeds when the frame turns to the
/// terminal page after the input.
const REGION_TASK: &[u8] = br#"{
    "schema_version":"0.6",
    "task_id":"task",
    "game":"neutral",
    "server_scope":["test"],
    "coordinate_space":{"width":2,"height":1},
    "entry_page":"home",
    "target_page":"terminal",
    "operations":[{
        "id":"open_terminal",
        "from":"home",
        "click":{"kind":"rect","x":0,"y":0,"width":2,"height":1},
        "unguarded_trusted_coordinate":true,
        "retryable":false
    }]
}"#;

// ---- The fixture.

/// The fixture catalog: `fixture.observe` due at every evaluation, and its twin on the same
/// procedure at a lower priority; an activity window of the whole day without an interval.
fn catalog() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let observe = &mut tasks["tasks"][0];
    observe["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1, "anchor_ms": 0}
    });
    observe["cooldown_ms"] = json!(0);
    observe["expected_duration_ms"] = json!(DECLARED_MS);
    observe["loop_budget"] = json!({
        "daily_limit": 1000,
        "window_iteration_limit": 1000,
        "max_runtime_ms": MAX_RUNTIME_MS
    });
    observe["on_failure"]["escalation_threshold"] = json!(1000);
    let mut twin = tasks["tasks"][0].clone();
    twin["id"] = json!(TWIN);
    twin["priority"] = json!(50);
    twin["produces"] = json!([]);
    twin["instance_overrides"] = json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("fixture tasks array")
        .push(twin);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let profile = &mut activity["profiles"][0];
    profile["windows"] = json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    profile["daily_budget"] = json!(1000);
    profile["max_window_iterations"] = json!(1000);
    profile["session_max_ms"] = json!(86_400_000);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("fixture activity bytes");
    sources
}

/// The procedure bindings: the fixture procedure at `package`'s location.
fn manifest(package: &Path) -> ProcedureManifest {
    ProcedureManifest::new([PROCEDURE, "procedure.observe-b", "procedure.detect"].map(
        |procedure_ref| {
            let binding = ProcedureBinding::new(
                procedure_ref,
                "operation.observe",
                vec!["after_observation".to_owned()],
            )
            .expect("procedure binding");
            if procedure_ref == PROCEDURE {
                binding.with_package_path(package.to_string_lossy())
            } else {
                binding
            }
        },
    ))
    .expect("procedure manifest")
}

/// The configuration with `scheduling.expected_runtime.enabled` set to `enabled`.
fn switch(enabled: bool) -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: actingcommand_runtime_config::scheduling::EXPECTED_RUNTIME_ENABLED.to_owned(),
        value: FactScalar::Boolean(enabled),
        source: ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// One host on `root` and its pieces, which a restart reuses.
struct Bench {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    instance: InstanceId,
    package: PathBuf,
    /// Whether the instance is a device fake rather than the fixture simulation, which refuses
    /// a client's run (`fixture_execution_scope_forbidden`).
    physical: bool,
}

impl Bench {
    /// A root holding the neutral package (`package` its bytes), at the fixture's time, on the
    /// fixture simulation.
    fn new(package: &[u8]) -> Self {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("ed2-task.zip");
        fs::write(&path, package).expect("write the package");
        Self {
            root,
            clock: Arc::new(ManualRuntimeClock::new(
                POLICY_NOW_UNIX_MS,
                POLICY_NOW_UNIX_MS,
            )),
            state: Arc::new(FakeState::default()),
            instance: instance_id(),
            package: path,
            physical: false,
        }
    }

    /// The same on a device fake whose display is observed, as a direct run needs (Workflow
    /// #392 DA-A), holding the neutral package with the device's task geometry; its frame
    /// turns to the terminal page after the input.
    fn physical() -> Self {
        let mut bench = Self::new(&neutral_region_contained_task_package(true));
        bench.physical = true;
        bench
            .state
            .physical_task_geometry
            .store(true, Ordering::Release);
        bench
            .state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        bench
    }

    fn path(&self) -> String {
        self.package.to_string_lossy().into_owned()
    }

    /// A host with the fixture catalog active; `configuration` the effective configuration's
    /// explicit keys, when there are any.
    fn start(&self, configuration: Option<RuntimeConfigManifest>) -> RuntimeHost {
        let (hold, _release, _finished) = PackAdmissionHold::open();
        let mut host_config = config(&self.root)
            .with_runtime_clock(self.clock.clone())
            .with_procedure_manifest(manifest(&self.package))
            .with_pack_admission_hold_for_test(hold);
        if let Some(configuration) = configuration {
            host_config = host_config.with_config_manifest(configuration);
        }
        let provider = FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            self.instance,
            Arc::clone(&self.state),
        );
        let provider = if self.physical {
            provider
        } else {
            provider.fixture_simulation()
        };
        let host = RuntimeHost::start(host_config, Arc::new(provider)).expect("runtime host");
        host.activate_policy_catalog(&catalog())
            .expect("activate the fixture catalog");
        host
    }

    /// Evaluates at `at` (the Runtime clock moved there first).
    fn evaluate(&self, host: &RuntimeHost, at: u64, seed: u64) -> PolicyEvaluation {
        advance_manual_clock_to(&self.clock, at);
        host.evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy")
        .evaluation
        .expect("an evaluation")
    }

    /// Moves the Runtime clock to `at`.
    fn at(&self, at: u64) {
        advance_manual_clock_to(&self.clock, at);
    }
}

/// The evaluation's one dispatch on the fixture instance and its reason chain. The two tasks
/// share the package, so either one's dispatch reserves the same E; which one the ranking picks
/// (the waiting one ages) decides nothing here.
fn dispatch(evaluation: &PolicyEvaluation) -> (DispatchIntent, DecisionReasonChain) {
    let [intent] = evaluation.dispatch_intents.as_slice() else {
        panic!("one dispatch: {evaluation:#?}")
    };
    assert!(
        intent.task_id == OBSERVE || intent.task_id == TWIN,
        "{intent:?}"
    );
    let intent = intent.clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch reason chain")
        .clone();
    (intent, chain)
}

/// Approves the catalog and admits the dispatch, which must be granted.
fn admit(
    host: &RuntimeHost,
    intent: &DispatchIntent,
    chain: &DecisionReasonChain,
) -> Box<PolicyRunContext> {
    record_policy_approval(host, intent);
    // A capacity sample is fresh only for a while on the Runtime clock, which the tests move.
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(intent, chain, &policy_context(host, intent))
        .expect("the dispatch is admitted")
    else {
        panic!("expected a granted dispatch")
    };
    context
}

/// The package's samples on the fixture instance, newest first.
fn samples(host: &RuntimeHost) -> Vec<u64> {
    host.run_durations_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the run-duration record")
        .samples(PACKAGE_ID)
        .to_vec()
}

/// E of `samples`, newest first, by the model's formula.
fn expected_of(samples: &[u64]) -> u64 {
    let used = &samples[..samples.len().min(RUNS)];
    let sum = used.iter().sum::<u64>();
    sum.div_ceil(used.len() as u64) + MARGIN_MS
}

/// The `fact.published` records of the key.
fn published(host: &RuntimeHost) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::FactPublished),
        ..EventQuery::default()
    })
    .expect("the published facts")
    .iter()
    .filter(|event| match event.payload() {
        EventPayload::Fact(actingcommand_contract::FactPayload::Published(payload)) => {
            payload.records().any(|record| record.key == KEY)
        }
        _ => false,
    })
    .count()
}

/// The `expected_duration_ms` member each dispatch intent record carries, in ledger order.
fn recorded_durations(host: &RuntimeHost) -> Vec<Option<u64>> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyDispatchIntent),
        ..EventQuery::default()
    })
    .expect("the dispatch intents")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => {
            Some(payload.expected_duration_ms())
        }
        _ => None,
    })
    .collect()
}

/// The `run_duration_row_dropped` warnings, in ledger order.
fn dropped(host: &RuntimeHost) -> Vec<actingcommand_contract::outcome::Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("the runtime failures")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(failed))
            if failed.outcome().code().as_str() == ROW_DROPPED =>
        {
            Some(failed.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// The answer of a ScheduleExplanation read with candidate rows.
fn explanation(host: &RuntimeHost) -> RuntimeScheduleExplanation {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ScheduleExplanation {
        instance: Some(POLICY_INSTANCE_ALIAS.to_owned()),
        candidates: true,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
        panic!("expected a schedule explanation: {:?}", receipt.result());
    };
    (**explanation).clone()
}

/// The candidate row of the task `intent` did not dispatch (the other one, which waited this
/// round): its expected duration and where it comes from.
fn other_row(host: &RuntimeHost, intent: &DispatchIntent) -> (Option<u64>, Option<String>) {
    let other = if intent.task_id == OBSERVE {
        TWIN
    } else {
        OBSERVE
    };
    let explanation = explanation(host);
    let row = explanation
        .instances
        .iter()
        .flat_map(|entry| entry.candidates.iter())
        .find(|row| row.task_id == other)
        .unwrap_or_else(|| panic!("{other} has a candidate row: {explanation:#?}"));
    (row.expected_runtime_ms, row.expected_runtime_source.clone())
}

/// Adds the samples `durations` (in that order, the last the newest), each run ending one second
/// after the previous one from `from`.
fn add_samples(bench: &Bench, host: &RuntimeHost, durations: &[u64], from: u64) -> u64 {
    let mut at = from;
    for duration in durations {
        at += 1_000;
        bench.at(at);
        host.add_run_duration_sample_for_test(POLICY_INSTANCE_ALIAS, PACKAGE_ID, *duration, at)
            .expect("record the sample");
    }
    at
}

// ---- The tests.

// ED-2 row, "a success adds a sample to its package row": a scheduled run that succeeds adds its
// duration (the `runtime_ms` of its execution record) as the newest sample of its package's row,
// with the run's terminal time as the row's `at`, in one Runtime publication of the record. Its
// next evaluation dispatches with E, and the other task, which shares the package, is shown with
// E and `runs:1` (before the run: the declared duration, `declared`).
#[test]
fn ed2_t1_a_success_adds_a_sample_to_its_package_row() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    bench
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = bench.start(None);
    let evaluation = bench.evaluate(&host, POLICY_NOW_UNIX_MS, 11);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, DECLARED_MS);
    assert_eq!(
        other_row(&host, &intent),
        (Some(DECLARED_MS), Some("declared".to_owned()))
    );
    assert!(samples(&host).is_empty());

    let context = admit(&host, &intent, &chain);
    let request = ContainedTaskRequest::new(bench.path()).expect("contained task request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the scheduled run succeeds");
    let (execution, _) = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("the scheduled run completes");
    let runtime_ms = match &execution.outcome {
        PolicyExecutionOutcome::Succeeded { runtime_ms } => *runtime_ms,
        other => panic!("expected a success: {other:?}"),
    };
    let durations = host
        .run_durations_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the run-duration record");
    let row = durations.row(PACKAGE_ID).expect("the package's row");
    assert_eq!(row.samples, vec![runtime_ms]);
    assert_eq!(row.at_unix_ms, execution.observed_at_unix_ms);
    assert_eq!(durations.len(), 1);
    assert_eq!(published(&host), 1, "one publication of the record");

    let next = bench.evaluate(&host, execution.observed_at_unix_ms + HOUR_MS, 12);
    let (again, _) = dispatch(&next);
    let expected = expected_of(&[runtime_ms]);
    assert_eq!(again.expected_duration_ms, expected);
    assert_eq!(
        other_row(&host, &again),
        (Some(expected), Some("runs:1".to_owned()))
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ED-2 row, "a failure, condition end, direct run or Lab run adds none": a scheduled run that
// fails, one that a condition ends and a direct run of the same package add no sample, and the
// record is never published. A Lab operation never reaches a policy settlement, the only writer.
#[test]
fn ed2_t2_a_failure_a_condition_end_and_a_direct_run_add_none() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    let request = ContainedTaskRequest::new(bench.path()).expect("contained task request");

    // A failure: the frame never turns, so the run exhausts its steps.
    let evaluation = bench.evaluate(&host, POLICY_NOW_UNIX_MS, 21);
    let (intent, chain) = dispatch(&evaluation);
    let context = admit(&host, &intent, &chain);
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{error:?}");
    assert!(samples(&host).is_empty());

    // A run a condition ends.
    let evaluation = bench.evaluate(&host, POLICY_NOW_UNIX_MS + HOUR_MS, 22);
    let (intent, chain) = dispatch(&evaluation);
    let context = admit(&host, &intent, &chain);
    host.settle_next_policy_run_by_condition_for_test(PolicyConditionSettlement {
        refunded: false,
        count: 1,
        expires_at_unix_ms: POLICY_NOW_UNIX_MS + 3 * HOUR_MS,
    })
    .expect("the condition of the next settlement");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run ends");
    assert!(!error.is_fatal(), "{error:?}");
    let ended = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyExecutionRecorded),
            ..EventQuery::default()
        })
        .expect("the execution records")
        .iter()
        .any(|event| {
            matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
                    if payload.decision_id() == intent.decision_id
                        && matches!(
                            payload.policy_result(),
                            PolicyExecutionOutcome::Failed { failure }
                                if failure.condition.is_some()
                        )
            )
        });
    assert!(ended, "the second run was settled by its condition");
    assert!(samples(&host).is_empty());
    assert_eq!(published(&host), 0, "the record was never published");
    host.close().expect("close the host");

    // A direct run of the same package that succeeds. The fixture simulation refuses a client's
    // run, so it runs on a device fake with the same catalog, the switch on and the package's id
    // at its location known: a scheduled success there would add a sample.
    let bench = Bench::physical();
    let host = bench.start(None);
    host.learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    let mut client = TestClient::connect(&host);
    let direct = client.request(RuntimeOperation::run_contained_task(
        POLICY_INSTANCE_ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(bench.path()).expect("contained task request"),
    ));
    let receipt = client.send(&direct);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(samples(&host).is_empty());
    assert_eq!(published(&host), 0, "the record was never published");
    drop(client);
    host.close().expect("close the host");
}

// ED-2 row, "the sixth sample drops the first": a row keeps the newest `runs` (5) samples, and
// the evaluation's E is the mean of exactly those, rounded up, plus the margin.
#[test]
fn ed2_t3_the_sixth_sample_drops_the_first() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    host.learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    let at = add_samples(
        &bench,
        &host,
        &[1_000, 2_000, 3_000, 4_000, 5_001],
        POLICY_NOW_UNIX_MS,
    );
    assert_eq!(samples(&host), vec![5_001, 4_000, 3_000, 2_000, 1_000]);
    let evaluation = bench.evaluate(&host, at + 1_000, 31);
    let (intent, _) = dispatch(&evaluation);
    // ⌈15 001 / 5⌉ + 300 000.
    assert_eq!(intent.expected_duration_ms, 3_001 + MARGIN_MS);

    let at = add_samples(&bench, &host, &[6_000], at + 1_000);
    assert_eq!(samples(&host), vec![6_000, 5_001, 4_000, 3_000, 2_000]);
    let evaluation = bench.evaluate(&host, at + 1_000, 32);
    let (intent, _) = dispatch(&evaluation);
    // ⌈20 001 / 5⌉ + 300 000.
    assert_eq!(intent.expected_duration_ms, 4_001 + MARGIN_MS);
    host.close().expect("close the host");
}

// ED-2 row, "restart keeps the samples" (R6.3: the record is in the ledger only and the start
// loads it from the tag index before the first evaluation), with the ruling of 10-24 14:1x: a
// dispatch admitted with E records E on its intent, and the restart that recovers it reserves
// that value again (no `policy_budget_receipt_mismatch`); after the restart the evaluation's E
// and the admission's agree. The dispatch is settled through the policy API, which no scheduled
// path prepared, so it adds no sample.
#[test]
fn ed2_t4_restart_keeps_the_samples_and_recovers_a_dispatch_admitted_with_e() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    host.learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    let at = add_samples(&bench, &host, &[40_000, 20_000], POLICY_NOW_UNIX_MS);
    let expected = expected_of(&[20_000, 40_000]);
    let evaluation = bench.evaluate(&host, at + 1_000, 41);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, expected);
    let context = admit(&host, &intent, &chain);
    assert_eq!(
        context.admission().budget.task_runtime_reserved_ms,
        expected,
        "the admission reserved E"
    );
    assert_eq!(recorded_durations(&host), vec![Some(expected)]);
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("settle the dispatch");
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    assert_eq!(samples(&host), vec![20_000, 40_000]);
    host.close().expect("close the first host");

    bench.at(at + HOUR_MS);
    let restarted = bench.start(None);
    assert!(
        restarted
            .fatal_error()
            .expect("read the fatal state")
            .is_none(),
        "the dispatch admitted with E is recovered"
    );
    assert_eq!(samples(&restarted), vec![20_000, 40_000]);
    restarted
        .learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    let evaluation = bench.evaluate(&restarted, at + 2 * HOUR_MS, 42);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, expected);
    admit(&restarted, &intent, &chain);
    assert!(
        restarted
            .fatal_error()
            .expect("read the fatal state")
            .is_none()
    );
    restarted.close().expect("close the restarted host");
}

// ED-2 row, "a package update keeps them" (R6.2): the package changed in place (same
// `package_id`, other content) keeps its row; its next successful run adds its newest sample to
// the earlier ones.
#[test]
fn ed2_t5_a_package_update_keeps_the_samples() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    bench
        .state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = bench.start(None);
    let at = add_samples(&bench, &host, &[30_000, 10_000], POLICY_NOW_UNIX_MS);
    assert_eq!(samples(&host), vec![10_000, 30_000]);

    let updated = neutral_contained_task_package_with_task_and_timeout(false, REGION_TASK, 6_000);
    assert_ne!(
        updated,
        neutral_region_contained_task_package(false),
        "the update changes the package's content"
    );
    fs::write(&bench.package, &updated).expect("update the package in place");
    let evaluation = bench.evaluate(&host, at + 1_000, 51);
    let (intent, chain) = dispatch(&evaluation);
    let context = admit(&host, &intent, &chain);
    let request = ContainedTaskRequest::new(bench.path()).expect("contained task request");
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the updated package runs");
    let (execution, _) = host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("the run completes");
    let runtime_ms = match &execution.outcome {
        PolicyExecutionOutcome::Succeeded { runtime_ms } => *runtime_ms,
        other => panic!("expected a success: {other:?}"),
    };
    assert_eq!(samples(&host), vec![runtime_ms, 10_000, 30_000]);
    host.close().expect("close the host");
}

// ED-2 row, "a client publish is refused `run_duration_key_reserved`": an agent's PublishFact of
// the key, and the Runtime's own ordinary publication path, are refused with the key as a
// non-fatal request error, and nothing is appended.
#[test]
fn ed2_t6_a_client_publish_of_the_key_is_refused() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    let record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        KEY,
        ContractFactValue::RecordList(Vec::new()),
        "snapshot:ed2-client",
        Vec::new(),
    );
    let mut client = TestClient::connect(&host);
    let request = client.agent_request(RuntimeOperation::PublishFact {
        record: record.clone(),
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        KEY_RESERVED,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().value("fact_key"),
        Some(&json!(KEY)),
        "{receipt:?}"
    );
    let error = host
        .publish_fact(record)
        .expect_err("only a settlement writes the key");
    assert_eq!(error.code(), KEY_RESERVED, "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(published(&host), 0, "nothing is appended");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the host");
}

// ED-2 row, "row 257 drops the oldest with a warning": a 257th package's row drops the row with
// the oldest `at` and records one `run_duration_row_dropped` (warning) naming that package and
// the instance; the record keeps 256 rows.
#[test]
fn ed2_t7_row_257_drops_the_oldest_with_a_warning() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    bench.at(POLICY_NOW_UNIX_MS + 1_000);
    for index in 0..=256_u64 {
        host.add_run_duration_sample_for_test(
            POLICY_INSTANCE_ALIAS,
            &format!("neutral.ed2.package-{index:03}"),
            1_000 + index,
            POLICY_NOW_UNIX_MS + index,
        )
        .expect("record the sample");
        if index < 256 {
            assert!(dropped(&host).is_empty(), "no row dropped at row {index}");
        }
    }
    let warnings = dropped(&host);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(
        warnings[0].value("package_id"),
        Some(&json!("neutral.ed2.package-000"))
    );
    assert_eq!(
        warnings[0].value("instance_id"),
        Some(&json!(POLICY_INSTANCE_ALIAS))
    );
    let durations = host
        .run_durations_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the run-duration record");
    assert_eq!(durations.len(), 256);
    assert!(durations.row("neutral.ed2.package-000").is_none());
    assert_eq!(
        durations.samples("neutral.ed2.package-256"),
        &[1_256_u64][..]
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ED-2 row, "an intent stale after a new sample is refused `policy_facts_stale`": a sample that
// lands between the evaluation and the admission changes the pair's E, so the intent evaluated
// before it is refused; the next evaluation's intent, with the new E, is admitted.
#[test]
fn ed2_t8_an_intent_stale_after_a_new_sample_is_refused() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    host.learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    let at = add_samples(&bench, &host, &[1_000], POLICY_NOW_UNIX_MS);
    let evaluation = bench.evaluate(&host, at + 1_000, 81);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, expected_of(&[1_000]));
    record_policy_approval(&host, &intent);

    let at = add_samples(&bench, &host, &[3_000], at + 1_000);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let refused = host
        .admit_policy_dispatch(&intent, &chain, &policy_context(&host, &intent))
        .expect_err("an intent evaluated before the sample is stale");
    assert_eq!(refused.code(), STALE, "{refused:?}");

    let evaluation = bench.evaluate(&host, at + 1_000, 82);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, expected_of(&[3_000, 1_000]));
    admit(&host, &intent, &chain);
    host.close().expect("close the host");
}

// ED-2 row, `scheduling.expected_runtime.enabled = false` (§3.4 "Configuration": the rollback
// without a restart of the code): no sample is written, the record kept from before is not read,
// and the evaluation is the one without samples: every dispatch reserves the declared duration,
// its intent records no duration, and `ac_schedule` shows `declared`.
#[test]
fn ed2_t9_with_the_switch_off_nothing_is_written_or_read() {
    let bench = Bench::new(&neutral_region_contained_task_package(false));
    let host = bench.start(None);
    let at = add_samples(&bench, &host, &[2_000, 4_000], POLICY_NOW_UNIX_MS);
    host.close().expect("close the host with the switch on");

    bench.at(at + HOUR_MS);
    let off = bench.start(Some(switch(false)));
    off.learn_package_for_test(&bench.path(), PACKAGE_ID)
        .expect("the package's id at its location");
    assert_eq!(samples(&off), vec![4_000, 2_000], "the record is kept");
    let records = published(&off);
    off.add_run_duration_sample_for_test(POLICY_INSTANCE_ALIAS, PACKAGE_ID, 8_000, at + HOUR_MS)
        .expect("a sample with the switch off");
    assert_eq!(samples(&off), vec![4_000, 2_000], "no sample is written");
    assert_eq!(published(&off), records, "nothing is published");

    let evaluation = bench.evaluate(&off, at + 2 * HOUR_MS, 91);
    let (intent, chain) = dispatch(&evaluation);
    assert_eq!(intent.expected_duration_ms, DECLARED_MS);
    assert_eq!(
        other_row(&off, &intent),
        (Some(DECLARED_MS), Some("declared".to_owned()))
    );
    let context = admit(&off, &intent, &chain);
    assert_eq!(
        context.admission().budget.task_runtime_reserved_ms,
        DECLARED_MS
    );
    assert_eq!(recorded_durations(&off), vec![None]);
    off.close().expect("close the host with the switch off");
}
