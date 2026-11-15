// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 K1 (register §1.395; contracts/linear-steps.md, "What the program saw"; the
// coordinator's work order `rd-1115/brief-469-word-slices.md`), the host's half: the word a
// `linear_steps` step's end carries (`confirmation_problem`) reaches a scheduled run's policy
// execution record with its outcome, and changes neither the failure identity its settlement writes
// nor the streak that counts it. The identity's base is the failure's code, and its key K is the one
// of that code, the scope `main`, the step `open` and detail′ of the detail the kernel gives that
// end, which holds no word (contracts/policy-suspension.md, "Failure identity"); the same failure
// again, with the same word, keeps the first identity byte for byte and counts on, as the streak
// compares the code, the identity and the class only. The kernel's half (`execution-kernel`,
// `contained_task::linear::gate_469_k1`) pins each word, the code, the detail, the timing and the
// other values of every end.
//
// The package and the device are U1's (`gate_463_u1`): a `linear_steps` source package of game
// `neutral`, server `test`, 2 x 1, one tap `open` from `home` (first pixel red, as the fake device
// shows it) to `list` (first pixel blue), expected within 200 ms and attempted twice, 10 ms apart;
// the retry decision and the closing decision each wait the step timeout of 600 ms, a frame every
// 50 ms. After the tap the device keeps `home` (the tap swallowed) or shows a black first pixel (a
// screen of neither page, unchanged frame after frame). The fixture catalog's one task
// `fixture.observe` runs it on `fixture-instance-a`, on the test's manual clock.

use super::*;
use crate::failure_identity::{FailureIdentity, failure_detail_fingerprint, failure_key};
use serde_json::{Value, json};

const UNRECORDED_SCREEN: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen
        .as_str();
const CONFIRMATION_FAILED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.as_str();
// The fixture catalog's one task and its procedure.
const TASK: &str = "fixture.observe";
const PROCEDURE: &str = "procedure.observe";
// The details the kernel gives the two ends (contracts/linear-steps.md, "Failure codes"): the
// second attempt's after the swallowed taps, and the first attempt's after its retry decision.
const SWALLOWED_DETAIL: &str =
    "operation=open attempts=2 after_page=<unrecognized> hit_error_page=false transition=none";
const UNRECORDED_DETAIL: &str =
    "operation=open attempts=1 after_page=<unrecognized> hit_error_page=false transition=none";

// What the device shows after the tap.
#[derive(Debug, Clone, Copy)]
enum AfterTap {
    // A black first pixel on every frame: neither `list` nor `home`, unchanged.
    Unrecorded,
    // `home` on every frame: the tap is swallowed.
    OwnPage,
}

// Writes the package as a source package directory at `directory`.
fn write_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.k1.open",
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
    let operation = json!({
        "id": "open",
        "from": "home",
        "to": "list",
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "expect_after": {"page_id": "list", "timeout_ms": 200},
        "retryable": true,
        "max_attempts": 2,
        "retry_interval_ms": 10
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "K1 fixture",
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
    if let AfterTap::Unrecorded = after {
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, inputs: usize| {
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
// procedure (`scheduled_execution.package_path`).
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

// A host on the test's clock with the task's procedure bound at `package` and the instance's fake
// device `state`, a fixture simulation for a scheduled run.
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

// The recommended `on_failure` of a linear task (contracts/policy-suspension.md, "Catalog"): one
// retry at once, the task paused by the second same failure.
fn recommended() -> Value {
    json!({"action": "pause", "retry_limit": 1, "retry_backoff_ms": 0, "escalation_threshold": 2})
}

// `on_failure` `continue` with no retry.
fn continue_without_retry() -> Value {
    json!({"action": "continue", "retry_limit": 0, "retry_backoff_ms": 0, "escalation_threshold": 3})
}

// The fixture catalog, its task's `on_failure` replaced by `on_failure`.
fn sources(on_failure: Value) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("policy tasks");
    tasks["tasks"][0]["on_failure"] = on_failure;
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
    // A capacity sample is fresh for twice the monitor's interval on the Runtime clock, and
    // admission refuses a stale one, so capacity is sampled at the new time.
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

// R22's rerun time: the retry backoff and the activity's sampled interval after the admission of
// `context` have both passed.
fn rerun_time(
    failure: &actingcommand_contract::PolicyFailureRecord,
    context: &PolicyRunContext,
) -> u64 {
    failure
        .retry_at_unix_ms
        .expect("a failure scheduled for a retry has its retry time")
        .max(context.admission().activity.next_eligible_unix_ms)
}

// The failure identity `failure` records for the run that failed with `error`, whose detail is
// `detail`: its base is the failure's code, and its key is the one of that code, the scope `main`,
// the step `open` and detail′ of `detail`, none of which holds the word. Returns the identity as
// recorded.
fn identity_without_the_word(
    case: &str,
    failure: &actingcommand_contract::PolicyFailureRecord,
    error: &RuntimeHostError,
    detail: &str,
) -> String {
    assert_eq!(error.native_detail(), Some(detail), "{case}");
    let code = actingcommand_contract::scheduled_failure_code_str(&failure.outcome);
    assert_eq!(code.as_str(), error.code(), "{case}");
    let recorded = failure
        .failure_identity
        .clone()
        .unwrap_or_else(|| panic!("{case}: a failure identity: {failure:?}"));
    let identity = FailureIdentity::parse(&recorded)
        .unwrap_or_else(|| panic!("{case}: a format v2 failure identity: {recorded}"));
    assert_eq!(identity.base, error.code(), "{case}");
    assert_eq!(
        identity.key,
        failure_key(
            &code,
            "main",
            "open",
            &failure_detail_fingerprint(Some(detail))
        ),
        "{case}"
    );
    recorded
}

// The swallowed tap, under the recommended `on_failure`: the first scheduled run fails
// `page_confirmation_failed` after two taps, its policy record's outcome carrying
// `confirmation_problem` `input_not_taken`, its identity the one of the code and the detail alone,
// counted once and retried at once. The retry fails the same way with the same word: its identity
// is the first one byte for byte, counted twice, and the second same failure pauses the pair.
#[test]
fn k1_a_swallowed_tap_s_word_leaves_the_failure_identity_and_the_streak_as_they_are() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("k1-open");
    write_package(&package);
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = device(AfterTap::OwnPage);
    let host = start(&root, &clock, &package, &state);
    host.activate_policy_catalog(&sources(recommended()))
        .expect("activate policy catalog");

    let (first_context, first_error) =
        failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(first_error.code(), CONFIRMATION_FAILED);
    assert_eq!(state.input_count.load(Ordering::Acquire), 2);
    let first = recorded_failure(&host, first_context.run_id());
    assert_eq!(
        first.outcome.value("confirmation_problem"),
        Some(&json!("input_not_taken")),
        "{:?}",
        first.outcome
    );
    let first_identity =
        identity_without_the_word("the first failure", &first, &first_error, SWALLOWED_DETAIL);
    assert_eq!(first.consecutive_same_error, 1);
    assert_eq!(first.disposition, PolicyFailureDisposition::RetryScheduled);

    let rerun_at = rerun_time(&first, &first_context);
    let (second_context, second_error) = failed_scheduled_run(&host, &clock, &request, rerun_at, 8);
    assert_eq!(second_error.code(), CONFIRMATION_FAILED);
    let second = recorded_failure(&host, second_context.run_id());
    assert_eq!(
        second.outcome.value("confirmation_problem"),
        Some(&json!("input_not_taken")),
        "{:?}",
        second.outcome
    );
    assert_eq!(
        second.failure_identity.as_deref(),
        Some(first_identity.as_str()),
        "the same failure keeps the first identity"
    );
    assert_eq!(second.consecutive_same_error, 2);
    assert_eq!(second.disposition, PolicyFailureDisposition::PausedTask);
    host.close().expect("close runtime host");
}

// The screen the package never recorded, under `on_failure` `continue` with no retry: the scheduled
// run fails `contained_task_linear_unrecorded_screen` after one tap, its policy record's outcome
// carrying `confirmation_problem` `unrecorded_screen`, its identity the one of the code and the
// detail alone, counted once, and the pair paused at once as before.
#[test]
fn k1_an_unrecorded_screen_s_word_leaves_its_failure_identity_as_it_is() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("k1-open");
    write_package(&package);
    let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .expect("contained task request");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let state = device(AfterTap::Unrecorded);
    let host = start(&root, &clock, &package, &state);
    host.activate_policy_catalog(&sources(continue_without_retry()))
        .expect("activate policy catalog");

    let (context, error) = failed_scheduled_run(&host, &clock, &request, POLICY_NOW_UNIX_MS, 7);
    assert_eq!(error.code(), UNRECORDED_SCREEN);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let failure = recorded_failure(&host, context.run_id());
    assert_eq!(
        failure.outcome.value("confirmation_problem"),
        Some(&json!("unrecorded_screen")),
        "{:?}",
        failure.outcome
    );
    identity_without_the_word("the unrecorded screen", &failure, &error, UNRECORDED_DETAIL);
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
    host.close().expect("close runtime host");
}
