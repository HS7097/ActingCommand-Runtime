// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 U1 (register §1.372; contracts/linear-steps.md, "Failure codes";
// contracts/policy-suspension.md, "A failure that pauses at its first occurrence"; coordinator work
// order `rd-1114/brief-463-u1-unrecorded-screen.md`), the host's half: a linear step's retry
// decision that saw neither page on a settled screen ends the run with
// `contained_task_linear_unrecorded_screen`. The run's terminal and receipt name the frame of its
// `Failure` pin (`artifact_id`). A scheduled run that fails so pauses its pair (catalog task,
// instance) at once, whatever its `on_failure`, with no rerun; a changed package lifts the pause;
// another pair of the instance is dispatched as before; a direct run pauses nothing. Fix round 1:
// a step that declares no retry reaches the code through its closing decision, and the frame of
// the run's `Failure` pin is named the same way.
//
// The package is a `linear_steps` source package of game `neutral`, server `test`, 2 x 1: one tap
// `open` from `home` (first pixel red, as the fake device shows it) to `list` (first pixel blue),
// expected within 200 ms and attempted twice, 10 ms apart; the retry decision waits the step
// timeout of 600 ms, a frame every 50 ms. After the tap the device shows a black first pixel (a
// screen of neither page, unchanged frame after frame), or keeps `home` (the tap swallowed). The
// fixture catalog's one task `fixture.observe` runs it on `fixture-instance-a`, on the test's
// manual clock (test plan E-3: the kernel waits on it, a capture spends no time).

use super::instance_workers::all_events;
use super::*;
use crate::failure_identity::FailureIdentity;
use serde_json::{Value, json};

const UNRECORDED_SCREEN: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen
        .as_str();
const CONFIRMATION_FAILED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.as_str();
// The fixture catalog's one task, its procedure, and the task this file adds beside it.
const TASK: &str = "fixture.observe";
const PROCEDURE: &str = "procedure.observe";
const OTHER_TASK: &str = "fixture.other";
// The trigger's next hourly clock occurrence after the first run (the clock predicate's local
// interval, anchored at 0, has an occurrence at POLICY_NOW_UNIX_MS).
const NEXT_OCCURRENCE_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;
// The default least time between two lift-check measurements of one package.
const LIFT_INTERVAL_MS: u64 = 60_000;

// What the device shows after the tap.
#[derive(Debug, Clone, Copy)]
enum AfterTap {
    // A black first pixel on every frame: neither `list` nor `home`, unchanged.
    Unrecorded,
    // `home` on every frame: the tap is swallowed.
    OwnPage,
}

// Writes the package as a source package directory at `directory`, its taps
// `retry_interval_ms` apart (another value is other content).
fn write_package(directory: &Path, retry_interval_ms: u64) {
    write_package_with(directory, Some(retry_interval_ms));
}

// Fix round 1: the same package, its tap attempted twice `retry_interval_ms` apart, or, without
// one, declaring no retry.
fn write_package_with(directory: &Path, retry_interval_ms: Option<u64>) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.u1.open",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1
    });
    let probe = |id: &str, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let mut operation = json!({
        "id": "open",
        "from": "home",
        "to": "list",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "expect_after": {"page_id": "list", "timeout_ms": 200}
    });
    if let Some(retry_interval_ms) = retry_interval_ms {
        operation["retryable"] = json!(true);
        operation["max_attempts"] = json!(2);
        operation["retry_interval_ms"] = json!(retry_interval_ms);
    }
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "U1 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("first_red", [255, 0, 0]), probe("first_blue", [0, 0, 255])],
        "page_rules": {
            "home": {"required": ["first_red"]},
            "list": {"required": ["first_blue"]}
        },
        "entry_page": "home",
        "target_page": "list",
        "max_steps": 1,
        "operations": [operation]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("write package file");
    }
}

// The fake device: after the tap, the screen `after`.
fn device(after: AfterTap) -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    if matches!(after, AfterTap::Unrecorded) {
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(|_: usize, inputs: usize| {
                if inputs == 0 {
                    Vec::<(usize, [u8; 3])>::new()
                } else {
                    vec![(0_usize, [0_u8, 0, 0])]
                }
            }));
    }
    state
}

// The fixture manifest with the task's procedure bound at `package`, as actingd binds a scheduled
// procedure (`scheduled_execution.package_path`), so a lift check can measure it.
fn manifest_at(package: &Path) -> ProcedureManifest {
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

// A host on the test's clock with the task's procedure bound at `package` and the instance's
// fake device `state`, a fixture simulation for a scheduled run.
fn start(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    package: &Path,
    state: &Arc<FakeState>,
) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest_at(package)),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(state))
                .fixture_simulation(),
        ),
    )
    .expect("runtime host")
}

// The recommended `on_failure` of a linear task (contracts/policy-suspension.md, "Catalog").
fn recommended() -> Value {
    json!({"action": "pause", "retry_limit": 1, "retry_backoff_ms": 0, "escalation_threshold": 2})
}

// `on_failure` `continue` with no retry.
fn continue_without_retry() -> Value {
    json!({"action": "continue", "retry_limit": 0, "retry_backoff_ms": 0, "escalation_threshold": 3})
}

// The fixture catalog, its task's `on_failure` replaced by `on_failure` when given, and, with
// `other`, a second task of the same instance whose priority is lower, bound to the procedure no
// package is located for.
fn sources(on_failure: Option<Value>, other: bool) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("policy tasks");
    if let Some(on_failure) = on_failure {
        tasks["tasks"][0]["on_failure"] = on_failure;
    }
    if other {
        let mut second = tasks["tasks"][0].clone();
        second["id"] = json!(OTHER_TASK);
        second["procedure_ref"] = json!("procedure.observe-b");
        second["priority"] = json!(50);
        second["produces"] = json!([]);
        second["instance_overrides"] = json!([]);
        tasks["tasks"]
            .as_array_mut()
            .expect("the tasks array")
            .push(second);
    }
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("policy task bytes");
    sources
}

// Evaluates at `at` (the Runtime clock moved there first), admits the task's dispatch and runs it
// as a scheduled run that fails without poisoning the Runtime.
fn failed_scheduled_run(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    request: &ContainedTaskRequest,
    at: u64,
    seed: u64,
) -> (Box<PolicyRunContext>, RuntimeHostError) {
    advance_manual_clock_to(clock, at);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(host, PolicyTrigger::FactsChanged, at, seed);
    assert_eq!(intent.task_id, TASK);
    record_policy_approval(host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_scheduled_policy_dispatch(&intent, &reasons, &policy_context(host, &intent), request)
        .expect("policy admission")
    else {
        panic!("expected one policy run context")
    };
    let error = host
        .run_scheduled_contained_task(&context, request)
        .expect_err("the scheduled run fails");
    assert!(!error.is_fatal(), "{error:?}");
    (context, error)
}

// The failure the policy execution record of `run_id` settled.
fn recorded_failure(
    host: &RuntimeHost,
    run_id: RunId,
) -> actingcommand_contract::PolicyFailureRecord {
    let mut client = TestClient::connect(host);
    let events = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    drop(client);
    events
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => {
                    match payload.policy_result() {
                        PolicyExecutionOutcome::Failed { failure } => Some(failure.clone()),
                        PolicyExecutionOutcome::Succeeded { .. }
                        | PolicyExecutionOutcome::Stopped { .. } => None,
                    }
                }
                _ => None,
            },
            _ => None,
        })
        .expect("the failed execution record")
}

// What one evaluation decided: the tasks it dispatched on the instance, and whether a pause kept
// the task's pair.
struct Evaluated {
    dispatched: Vec<String>,
    paused: bool,
}

// Whether a pause kept the task's pair in `cycle`.
fn task_paused(cycle: &PolicyCycle) -> bool {
    cycle
        .evaluation
        .as_ref()
        .expect("an evaluation")
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .flat_map(|decision| decision.reasons.iter())
        .any(|reason| reason.outcome.code().as_str() == "policy_task_paused")
}

// Evaluates at `at` (the Runtime clock moved there first).
fn evaluate(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64, seed: u64) -> Evaluated {
    advance_manual_clock_to(clock, at);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: at,
                monotonic_ms: at,
            },
            seed,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate the policy");
    let paused = task_paused(&cycle);
    let dispatched = cycle
        .evaluation
        .as_ref()
        .expect("an evaluation")
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .map(|intent| intent.task_id.clone())
        .collect();
    Evaluated { dispatched, paused }
}

// Pauses its pair at once: under `on_failure` `continue` with no retry and under the recommended
// value, the run that meets the unrecorded screen fails with the new code, and its settlement
// pauses the pair (`paused_task`, class severe) with no retry time, writing a failure identity as
// an accumulating failure does (its error frame, not its decision); the pair is kept paused when it
// is due again. The swallowed tap in the same fixture fails with `page_confirmation_failed` and
// follows `on_failure` as before: `continue` with no retry, a retry scheduled under the
// recommended value.
#[test]
fn u1_a_scheduled_run_on_an_unrecorded_screen_pauses_its_pair_at_once() {
    for (case, on_failure, own_page_disposition) in [
        (
            "continue without retry",
            continue_without_retry(),
            PolicyFailureDisposition::Continue,
        ),
        (
            "the recommended value",
            recommended(),
            PolicyFailureDisposition::RetryScheduled,
        ),
    ] {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("u1-open");
        write_package(&package, 10);
        let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
            .expect("contained task request");

        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let state = device(AfterTap::Unrecorded);
        let host = start(&root, &clock, &package, &state);
        host.activate_policy_catalog(&sources(Some(on_failure.clone()), false))
            .expect("activate policy catalog");
        let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
        assert_eq!(error.code(), UNRECORDED_SCREEN, "{case}");
        assert_eq!(state.input_count.load(Ordering::Acquire), 1, "{case}");
        let failure = recorded_failure(&host, context.run_id());
        assert_eq!(
            failure.disposition,
            PolicyFailureDisposition::PausedTask,
            "{case}"
        );
        assert_eq!(failure.retry_at_unix_ms, None, "{case}: no rerun");
        assert_eq!(failure.consecutive_same_error, 1, "{case}");
        assert_eq!(failure.original_class, PolicyFailureClass::Severe, "{case}");
        assert_eq!(
            failure.effective_class,
            PolicyFailureClass::Severe,
            "{case}"
        );
        let identity = failure
            .failure_identity
            .as_deref()
            .and_then(FailureIdentity::parse)
            .unwrap_or_else(|| panic!("{case}: a failure identity: {failure:?}"));
        assert_eq!(identity.base, UNRECORDED_SCREEN, "{case}");
        assert!(identity.accumulates(), "{case}: {identity:?}");
        let due = evaluate(&host, &clock, NEXT_OCCURRENCE_UNIX_MS, 31);
        assert!(due.paused, "{case}: the pause keeps the pair");
        assert!(due.dispatched.is_empty(), "{case}: {:?}", due.dispatched);
        host.close().expect("close runtime host");

        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("u1-open");
        write_package(&package, 10);
        let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
            .expect("contained task request");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let state = device(AfterTap::OwnPage);
        let host = start(&root, &clock, &package, &state);
        host.activate_policy_catalog(&sources(Some(on_failure), false))
            .expect("activate policy catalog");
        let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
        assert_eq!(error.code(), CONFIRMATION_FAILED, "{case}");
        assert_eq!(state.input_count.load(Ordering::Acquire), 2, "{case}");
        let failure = recorded_failure(&host, context.run_id());
        assert_eq!(failure.disposition, own_page_disposition, "{case}");
        assert_eq!(
            failure.original_class,
            PolicyFailureClass::Recoverable,
            "{case}"
        );
        assert_eq!(failure.consecutive_same_error, 1, "{case}");
        host.close().expect("close runtime host");
    }
}

// Nothing else is affected: with a second task of the same instance in the catalog (its priority
// lower, so the first run is the paused task's), the first due evaluation after the pause keeps
// the paused pair and dispatches the other task on the instance, whose admission the instance
// grants (it is neither paused nor fenced) and which then settles; the next due evaluation keeps
// the pair paused and dispatches the other task again.
#[test]
fn u1_another_pair_of_the_instance_is_dispatched_while_the_pair_is_paused() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("u1-open");
    write_package(&package, 10);
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = device(AfterTap::Unrecorded);
    let host = start(&root, &clock, &package, &state);
    host.activate_policy_catalog(&sources(None, true))
        .expect("activate policy catalog");
    let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(error.code(), UNRECORDED_SCREEN);
    assert_eq!(
        recorded_failure(&host, context.run_id()).disposition,
        PolicyFailureDisposition::PausedTask
    );

    advance_manual_clock_to(&clock, NEXT_OCCURRENCE_UNIX_MS);
    host.capacity_sampler_for_test().expect("capacity owner")()
        .expect("capacity sample at the admission time");
    let (cycle, intent, reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::FactsChanged,
        NEXT_OCCURRENCE_UNIX_MS,
        41,
    );
    assert!(task_paused(&cycle), "the pause keeps the pair");
    assert_eq!(intent.task_id, OTHER_TASK);
    assert_eq!(intent.instance_id, POLICY_INSTANCE_ALIAS);
    let PolicyDispatchAdmission::Granted { .. } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the other task's admission")
    else {
        panic!("the instance grants the other task's dispatch")
    };
    host.complete_policy_dispatch(&intent.decision_id)
        .expect("the other task's dispatch settles");

    let next = evaluate(&host, &clock, NEXT_OCCURRENCE_UNIX_MS + 3_600_000, 42);
    assert!(next.paused, "the pause still keeps the pair");
    assert_eq!(next.dispatched, [OTHER_TASK]);
    host.close().expect("close runtime host");
}

// Lift: the pause keeps the pair when it is due again, and the package changed in place lifts it
// at the first due evaluation after the lift check's interval.
#[test]
fn u1_a_changed_package_lifts_the_pause() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("u1-open");
    write_package(&package, 10);
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = device(AfterTap::Unrecorded);
    let host = start(&root, &clock, &package, &state);
    host.activate_policy_catalog(&sources(Some(recommended()), false))
        .expect("activate policy catalog");
    let (_, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(error.code(), UNRECORDED_SCREEN);

    let unchanged = evaluate(&host, &clock, NEXT_OCCURRENCE_UNIX_MS, 51);
    assert!(unchanged.paused, "the package is unchanged");
    assert!(
        unchanged.dispatched.is_empty(),
        "{:?}",
        unchanged.dispatched
    );
    write_package(&package, 20);
    let changed = evaluate(
        &host,
        &clock,
        NEXT_OCCURRENCE_UNIX_MS + LIFT_INTERVAL_MS + 2_000,
        52,
    );
    assert!(!changed.paused, "the package changed");
    assert_eq!(changed.dispatched, [TASK]);
    host.close().expect("close runtime host");
}

// Raised, and a direct run: the run of the package on the instance, sent as `actingctl task-run`
// sends it, ends Failed with the new code after one tap. Its terminal and its receipt carry the
// kernel's values (the operation, the pages its retry decision awaited, the gate first and the
// step's own page last, its attempts) and `artifact_id`, the frame of the run's one `Failure` pin.
// The direct run pauses nothing: the task bound to the package is dispatched at its next due
// evaluation.
#[test]
fn u1_a_direct_run_names_the_pinned_frame_and_pauses_nothing() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("u1-open");
    write_package(&package, 10);
    let state = device(AfterTap::Unrecorded);
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(manifest_at(&package)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(463).expect("connection"))
        .expect("run receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "no input after the decision"
    );

    let events = all_events(&host);
    let failure_frames = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) => Some(summary),
            _ => None,
        })
        .flat_map(|summary| summary.summary().pinned())
        .filter(|pin| pin.reason() == PinnedFrameReason::Failure)
        .filter_map(|pin| pin.artifact())
        .map(|artifact| serde_json::to_value(artifact.artifact_id).expect("artifact id"))
        .collect::<Vec<_>>();
    let [failure_frame] = failure_frames.as_slice() else {
        panic!("one Failure pin with its frame: {failure_frames:?}");
    };
    let terminals = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal with its outcome: {terminals:?}");
    };
    for (carrier, outcome) in [("terminal", terminal), ("receipt", receipt.outcome())] {
        assert_eq!(outcome.code().as_str(), UNRECORDED_SCREEN, "{carrier}");
        let value = |key: &str| outcome.value(key).cloned();
        assert_eq!(value("pack_operation_id"), Some(json!("open")), "{carrier}");
        assert_eq!(
            value("page_ids"),
            Some(json!(["neutral/list", "neutral/home"])),
            "{carrier}"
        );
        assert_eq!(value("attempts"), Some(json!(1)), "{carrier}");
        assert_eq!(
            value("artifact_id").as_ref(),
            Some(failure_frame),
            "{carrier}"
        );
    }
    assert!(
        !events
            .iter()
            .any(|event| event.event_type() == EventType::PolicyExecutionRecorded),
        "a direct run settles no policy execution"
    );

    host.activate_policy_catalog(&sources(Some(recommended()), false))
        .expect("activate policy catalog");
    let due = evaluate(&host, &clock, NEXT_OCCURRENCE_UNIX_MS, 61);
    assert!(!due.paused, "nothing pauses the pair");
    assert_eq!(due.dispatched, [TASK]);
    host.close().expect("close runtime host");
}

// Fix round 1 (the coordinator's ruling on board #463; contracts/linear-steps.md, "Retries"): the
// same package whose tap declares no retry, run directly on the device instance. After the tap
// the device shows the screen of neither page; the arrival runs out and the step's closing
// decision, on `list` and then `home`, sees neither page on a settled screen: the run ends Failed
// with the new code after one tap, and its terminal and its receipt carry the kernel's values (the
// operation, the pages the closing decision awaited, the arrival's page first and the step's own
// page last, one attempt) and `artifact_id`, the frame of the run's one `Failure` pin.
#[test]
fn u1_a_step_without_a_retry_names_the_pinned_frame_of_its_closing_decision() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("u1-open-once");
    write_package_with(&package, None);
    let state = device(AfterTap::Unrecorded);
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock)
            .with_procedure_manifest(manifest_at(&package)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(463).expect("connection"))
        .expect("run receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        1,
        "the closing decision taps nothing"
    );

    let events = all_events(&host);
    let failure_frames = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) => Some(summary),
            _ => None,
        })
        .flat_map(|summary| summary.summary().pinned())
        .filter(|pin| pin.reason() == PinnedFrameReason::Failure)
        .filter_map(|pin| pin.artifact())
        .map(|artifact| serde_json::to_value(artifact.artifact_id).expect("artifact id"))
        .collect::<Vec<_>>();
    let [failure_frame] = failure_frames.as_slice() else {
        panic!("one Failure pin with its frame: {failure_frames:?}");
    };
    let terminals = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskFailed)
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted {
                    outcome: Some(outcome),
                    ..
                } => Some(outcome.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal with its outcome: {terminals:?}");
    };
    for (carrier, outcome) in [("terminal", terminal), ("receipt", receipt.outcome())] {
        assert_eq!(outcome.code().as_str(), UNRECORDED_SCREEN, "{carrier}");
        let value = |key: &str| outcome.value(key).cloned();
        assert_eq!(value("pack_operation_id"), Some(json!("open")), "{carrier}");
        assert_eq!(
            value("page_ids"),
            Some(json!(["neutral/list", "neutral/home"])),
            "{carrier}"
        );
        assert_eq!(value("attempts"), Some(json!(1)), "{carrier}");
        assert_eq!(
            value("artifact_id").as_ref(),
            Some(failure_frame),
            "{carrier}"
        );
    }
    host.close().expect("close runtime host");
}
