// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 U1b (0.12.63; register §1.389; contracts/policy-suspension.md, "A failure that
// pauses at its first occurrence"; contracts/linear-steps.md, "A screen the package never
// recorded"; coordinator work orders `rd-1115/brief-463-u1b-break-in.md` and
// `rd-1115/brief-463-u1b-fix2.md`): a scheduled run of a `linear_steps` task raises
// `contained_task_linear_unrecorded_screen`, and pauses its pair at once, only while its pair
// (catalog task, instance) is in the break-in of its package's content: until
// `scheduling.suspension.break_in_runs` runs of that content have settled as a success or a failure
// and `scheduling.suspension.break_in_ms` have passed since the first of them. After it the same
// closing verdict ends the run with `page_confirmation_failed`, settled as that code is (no pause at
// its first occurrence), the frame of the run's `Failure` pin still named as `artifact_id`. Either
// end carries Workflow #469 K1's word `confirmation_problem` `unrecorded_screen`, and the pause at
// once carries Workflow #469 H2's `pause_cause` `unrecorded_screen`. A stop and a condition end
// count nothing; a new content starts again; a restart keeps the count and the first time (the
// pair's value record), on the root's seeding start and on a loading start alike; a direct run
// raises the dedicated code always; the two keys set while the Runtime runs count from the next run.
// Fix round 3 (`rd-1115/brief-463-u1b-fix3.md`): a run whose prerequisite layer failed never began
// its package and counts nothing; a content its preparation could not measure and a break-in that
// could not be read each record one `policy_break_in_unread` with its `break_in_problem`, the run
// going on as one outside its break-in; `actingd suspended` lists the pair's break-in before and
// after it ends.
//
// The package is gate_463_u1's: a `linear_steps` source package of game `neutral`, 2 x 1, one tap
// `open` from `home` (first pixel red, the fake device's) to `list` (first pixel blue), attempted
// twice. After a run's tap the fake device shows `list` (the run succeeds), a black first pixel (a
// screen the package never recorded, still) or keeps `home` (the tap swallowed). The fixture
// catalog's task runs hourly; every run of a test is at the next hourly occurrence, on the test's
// manual clock. The expected runtime is off, so each dispatch reserves the task's declared
// duration.

use super::instance_workers::{all_events, wait_for_ladder_end};
use super::*;
use crate::codes::BreakInProblem;
use crate::host::ContainedTaskCheckpointPoint;
use crate::policy_control::PolicyValueRecord;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, PolicyBreakInRecord,
    PolicyConditionSettlement, RequestId, RuntimeConfigManifest, StartReportTaggedState,
    TaskEntryRecoveryRole,
};
use actingcommand_runtime_config::scheduling::{
    EXPECTED_RUNTIME_ENABLED, SUSPENSION_BREAK_IN_MS, SUSPENSION_BREAK_IN_RUNS,
};
use actingcommand_runtime_config::{DefaultsFile, mechanism_rows, render_parameters};
use serde_json::{Value, json};
use std::net::SocketAddr;

const UNRECORDED_SCREEN: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::ContainedTaskLinearUnrecordedScreen
        .as_str();
const CONFIRMATION_FAILED: &str =
    actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed.as_str();
const CANCELLED: &str = "contained_task_cancelled";
const BREAK_IN_UNREAD: &str = crate::codes::HostCode::PolicyBreakInUnread.as_str();
// The fixture catalog's one task and its procedure.
const TASK: &str = "fixture.observe";
const PROCEDURE: &str = "procedure.observe";
const HOUR_MS: u64 = 3_600_000;
// The runs' hourly occurrences.
const T0: u64 = POLICY_NOW_UNIX_MS;
const T1: u64 = T0 + HOUR_MS;
const T2: u64 = T0 + 2 * HOUR_MS;
const T3: u64 = T0 + 3 * HOUR_MS;
// The break-in most tests configure: two settled runs and an hour and a half since the first.
const RUNS: u32 = 2;
const BREAK_IN_MS: u64 = 5_400_000;
// How long a test waits for the stop's answer, which another thread gives.
const GATE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

// What the device shows after a run's tap.
#[derive(Debug, Clone, Copy)]
enum AfterTap {
    // A black first pixel on every frame: neither `list` nor `home`, unchanged.
    Unrecorded,
    // `home` on every frame: the tap is swallowed.
    OwnPage,
    // `list` on every frame: the step is confirmed.
    Reached,
}

// A color probe of the first pixel.
fn probe(id: &str, expected: [u8; 3]) -> Value {
    json!({"id": id,
           "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
           "expected": expected})
}

// A package's control of game `neutral`, 2 x 1, one step at most.
fn package_control(package_id: &str, execution_mode: &str) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": execution_mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1
    })
}

// Writes a source package directory at `directory`: its control, no resources, its one task.
fn write_files(directory: &Path, control: Value, task: Value) {
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

// Writes the package as a source package directory at `directory`, its taps `retry_interval_ms`
// apart (another value is another content); fix round 3: declaring the prerequisite package
// `prerequisite` when one is given.
fn write_package(directory: &Path, retry_interval_ms: u64) {
    write_package_with(directory, retry_interval_ms, None);
}

fn write_package_with(directory: &Path, retry_interval_ms: u64, prerequisite: Option<&str>) {
    let mut control = package_control("neutral.u1b.open", "linear_steps");
    if let Some(prerequisite) = prerequisite {
        control["prerequisite_package_id"] = json!(prerequisite);
    }
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "U1b fixture",
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
        "operations": [{
            "id": "open",
            "from": "home",
            "to": "list",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "list", "timeout_ms": 200},
            "retryable": true,
            "max_attempts": 2,
            "retry_interval_ms": retry_interval_ms
        }]
    });
    write_files(directory, control, task);
}

// Fix round 3: the prerequisite package `lead`, a page graph of game `neutral`, 2 x 1, from `away`
// (first pixel green) to `home` (first pixel red), one tap, declaring nothing. Listed in the game
// root of `neutral` (gate_373_nav3a's `StandardRoot`), it is the layer the gate runs when the
// screen is not the bench's package's first page; on a screen of neither of its pages it meets a
// page it does not know and fails, as gate_373_nav3a's failing prerequisite does.
const LEAD: &str = "neutral.u1b.lead";

fn write_lead(directory: &Path) {
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "U1b prerequisite fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("first_green", [0, 255, 0]), probe("first_red", [255, 0, 0])],
        "page_rules": {
            "away": {"required": ["first_green"]},
            "home": {"required": ["first_red"]}
        },
        "entry_page": "away",
        "target_page": "home",
        "operations": [{
            "id": "back",
            "from": "away",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    write_files(directory, package_control(LEAD, "navigable_route"), task);
}

// From the next tap on, the device shows `after`; before it, the fake device's own frame (`home`).
fn show_after_next_tap(state: &FakeState, after: AfterTap) {
    let taps_before = state.input_count.load(Ordering::Acquire);
    let first_pixel = match after {
        AfterTap::Unrecorded => [0_u8, 0, 0],
        AfterTap::OwnPage => [255_u8, 0, 0],
        AfterTap::Reached => [0_u8, 0, 255],
    };
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            if inputs > taps_before {
                vec![(0_usize, first_pixel)]
            } else {
                Vec::<(usize, [u8; 3])>::new()
            }
        }));
}

// Fix round 3: from now on every frame's first pixel is black, before any tap too: a screen
// neither the bench's package nor `lead` knows.
fn show_unknown_from_now(state: &FakeState) {
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(|_: usize, _: usize| vec![(0_usize, [0_u8, 0, 0])]));
}

// The fixture manifest with the task's procedure bound at `package`, as actingd binds a scheduled
// procedure (`scheduled_execution.package_path`).
fn procedures_at(package: &Path) -> ProcedureManifest {
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

fn parameter(key: &str, value: FactScalar) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source: ConfigParameterSource::Explicit,
        settable: false,
    }
}

// The break-in's two keys as actingd's assembly renders them for a file that does not name them:
// from the shipped defaults file, source `default` (fix round 2: a key the manifest does not hold
// is no setting of the configuration, so `ConfigureSettings` refused it `config_key_unknown`).
fn rendered_break_in_defaults() -> Vec<ConfigParameter> {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
        .expect("every key resolves")
        .into_iter()
        .filter(|row| row.key == SUSPENSION_BREAK_IN_RUNS || row.key == SUSPENSION_BREAK_IN_MS)
        .collect()
}

// The configuration: the expected runtime off and the break-in's two keys, as the file sets them
// when given, else at their defaults as the assembly renders them.
fn settings(break_in: Option<(u32, u64)>) -> RuntimeConfigManifest {
    let mut parameters = vec![parameter(
        EXPECTED_RUNTIME_ENABLED,
        FactScalar::Boolean(false),
    )];
    match break_in {
        Some((runs, ms)) => {
            parameters.push(parameter(
                SUSPENSION_BREAK_IN_RUNS,
                FactScalar::Integer(i64::from(runs)),
            ));
            parameters.push(parameter(
                SUSPENSION_BREAK_IN_MS,
                FactScalar::DurationMs(ms),
            ));
        }
        None => parameters.extend(rendered_break_in_defaults()),
    }
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// A host on `root` and the test's clock with the task's procedure bound at `package`, the instance
// `registered` on the fake device `state`: a fixture simulation for scheduled runs, or (`device`)
// a device instance whose display is observed, which a direct run and a stop's reset need
// (Workflow #392 DA-A). Fix round 3: with the standard-pack root `standard_packs` when one is given.
fn start(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    package: &Path,
    (state, registered, device): (&Arc<FakeState>, InstanceId, bool),
    manifest: RuntimeConfigManifest,
    standard_packs: Option<std::path::PathBuf>,
) -> RuntimeHost {
    let provider = if device {
        state.physical_task_geometry.store(true, Ordering::Release);
        *state
            .physical_task_extent
            .lock()
            .expect("fake physical task extent") = Some((2, 1));
        FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(state))
    } else {
        FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(state)).fixture_simulation()
    };
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_procedure_manifest(procedures_at(package))
            .with_config_manifest(manifest)
            .with_standard_packs_root(standard_packs),
        Arc::new(provider),
    )
    .expect("runtime host")
}

// The fixture catalog, its task's `on_failure` the recommended value of a linear task
// (contracts/policy-suspension.md, "Catalog").
fn sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: Value = serde_json::from_slice(&sources.tasks.bytes).expect("policy tasks");
    tasks["tasks"][0]["on_failure"] = json!(
        {"action": "pause", "retry_limit": 1, "retry_backoff_ms": 0, "escalation_threshold": 2}
    );
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("policy task bytes");
    sources
}

// One test's package, request, clock, fake device and instance; fix round 3: and its standard-pack
// root, none unless a test lists one.
struct Bench {
    root: TempDir,
    package: std::path::PathBuf,
    request: ContainedTaskRequest,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    registered: InstanceId,
    standard_packs: Option<std::path::PathBuf>,
}

impl Bench {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let package = root.path().join("u1b-open");
        write_package(&package, 10);
        let request = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
            .expect("contained task request");
        Self {
            root,
            package,
            request,
            clock: Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)),
            state: Arc::new(FakeState::default()),
            registered: instance_id(),
            standard_packs: None,
        }
    }

    // A host of the bench (`device`: on a device instance), its catalog active the first time.
    fn host(&self, device: bool, manifest: RuntimeConfigManifest, activate: bool) -> RuntimeHost {
        let host = start(
            &self.root,
            &self.clock,
            &self.package,
            (&self.state, self.registered, device),
            manifest,
            self.standard_packs.clone(),
        );
        if activate {
            host.activate_policy_catalog(&sources())
                .expect("activate policy catalog");
        }
        host
    }

    // Evaluates at `at` (the Runtime clock moved there first) and admits the task's dispatch as a
    // scheduled run.
    fn admitted(&self, host: &RuntimeHost, at: u64, seed: u64) -> Box<PolicyRunContext> {
        advance_manual_clock_to(&self.clock, at);
        host.capacity_sampler_for_test().expect("capacity owner")()
            .expect("capacity sample at the admission time");
        let (_, intent, reasons) =
            evaluated_policy_dispatch_at(host, PolicyTrigger::FactsChanged, at, seed);
        assert_eq!(intent.task_id, TASK);
        record_policy_approval(host, &intent);
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(host, &intent),
                &self.request,
            )
            .expect("policy admission")
        else {
            panic!("expected one policy run context")
        };
        context
    }

    // A scheduled run at `at` on `list` after its tap: it succeeds and settles as a success.
    fn succeed(&self, host: &RuntimeHost, at: u64, seed: u64) {
        show_after_next_tap(&self.state, AfterTap::Reached);
        let context = self.admitted(host, at, seed);
        let receipt = host
            .run_scheduled_contained_task(&context, &self.request)
            .expect("the scheduled run succeeds");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let (settled, _) = host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        assert!(
            matches!(settled.outcome, PolicyExecutionOutcome::Succeeded { .. }),
            "{:?}",
            settled.outcome
        );
    }

    // A scheduled run at `at` on `after` after its tap that fails without poisoning the Runtime:
    // its context, its error and the taps it made.
    fn fail(
        &self,
        host: &RuntimeHost,
        after: AfterTap,
        at: u64,
        seed: u64,
    ) -> (Box<PolicyRunContext>, RuntimeHostError, usize) {
        show_after_next_tap(&self.state, after);
        self.fail_as_shown(host, at, seed)
    }

    // Fix round 3: `fail` on the screen the device shows now.
    fn fail_as_shown(
        &self,
        host: &RuntimeHost,
        at: u64,
        seed: u64,
    ) -> (Box<PolicyRunContext>, RuntimeHostError, usize) {
        let taps_before = self.state.input_count.load(Ordering::Acquire);
        let context = self.admitted(host, at, seed);
        let error = host
            .run_scheduled_contained_task(&context, &self.request)
            .expect_err("the scheduled run fails");
        assert!(!error.is_fatal(), "{error:?}");
        let taps = self.state.input_count.load(Ordering::Acquire) - taps_before;
        (context, error, taps)
    }
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

// The pair's break-in as the host's decision memory holds it (its value record's member).
fn break_in(host: &RuntimeHost) -> Option<PolicyBreakInRecord> {
    let (held, _, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    match held.get(&format!("decision_memory/{POLICY_INSTANCE_ALIAS}/{TASK}")) {
        Some(PolicyValueRecord::Control(record)) => record.break_in.clone(),
        other => panic!("the pair's value record: {other:?}"),
    }
}

// The pause an evaluation at `at` (the Runtime clock moved there first) keeps the task's pair in,
// as its `policy_task_paused` reason's outcome; `None` when the pair is not paused.
fn pause_at(host: &RuntimeHost, clock: &ManualRuntimeClock, at: u64, seed: u64) -> Option<Outcome> {
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
        .find(|reason| reason.outcome.code().as_str() == "policy_task_paused")
        .map(|reason| reason.outcome.clone())
}

// The frames of the host's `Failure` pins (a success pins none), as artifact ids.
fn failure_frames(host: &RuntimeHost) -> Vec<Value> {
    let events = all_events(host);
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Capture(CapturePayload::SummaryCommitted(summary)) => Some(summary),
            _ => None,
        })
        .flat_map(|summary| summary.summary().pinned())
        .filter(|pin| pin.reason() == PinnedFrameReason::Failure)
        .filter_map(|pin| pin.artifact())
        .map(|artifact| serde_json::to_value(artifact.artifact_id).expect("artifact id"))
        .collect()
}

// The outcome of each failed terminal of the host's runs.
fn failed_terminals(host: &RuntimeHost) -> Vec<actingcommand_contract::outcome::Outcome> {
    let events = all_events(host);
    events
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
        .collect()
}

// `ConfigureSettings` of `key` to `value` on a new connection; it is taken.
fn configure(host: &RuntimeHost, key: &str, value: Value) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes: vec![ConfigSettingChange {
            key: key.to_owned(),
            value: Some(value),
        }],
        expected_version: None,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

// `StopInstanceRun` of `run` on the instance, answering at once, on a new connection.
fn stop_run(address: SocketAddr, run: RequestId) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::StopInstanceRun {
        instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
        request_id: run,
        wait_ms: 0,
    });
    client.send(&request)
}

// In its break-in (one settled run of two, an hour of an hour and a half) the run on the screen the
// package never recorded ends with the dedicated code after one tap and pauses its pair at once;
// it is settled as the break-in's second run. The recorded failure carries Workflow #469 K1's word
// `confirmation_problem` `unrecorded_screen`, and the pause Workflow #469 H2's `pause_cause`
// `unrecorded_screen` with the code as `failure_code`. (A new package, whose pair has no break-in
// yet, is gate_463_u1's `u1_a_scheduled_run_on_an_unrecorded_screen_pauses_its_pair_at_once`,
// unchanged.)
#[test]
fn u1b_t1_inside_the_break_in_the_code_pauses_its_pair_at_once() {
    let bench = Bench::new();
    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    let first = break_in(&host).expect("the first settled run starts the break-in");
    assert_eq!(first.runs, 1);
    let (context, error, taps) = bench.fail(&host, AfterTap::Unrecorded, T1, 8);
    assert_eq!(error.code(), UNRECORDED_SCREEN);
    assert_eq!(taps, 1);
    let failure = recorded_failure(&host, context.run_id());
    assert_eq!(failure.disposition, PolicyFailureDisposition::PausedTask);
    assert_eq!(failure.original_class, PolicyFailureClass::Severe);
    assert_eq!(
        failure.outcome.value("confirmation_problem"),
        Some(&json!("unrecorded_screen")),
        "{failure:?}"
    );
    let counted = break_in(&host).expect("the break-in");
    assert_eq!(counted.runs, 2, "the failure is the break-in's second run");
    assert_eq!(counted.content_hash, first.content_hash);
    assert_eq!(
        counted.first_settled_at_unix_ms,
        first.first_settled_at_unix_ms
    );
    let pause = pause_at(&host, &bench.clock, T2, 9).expect("the pause keeps the pair");
    assert_eq!(
        pause.value("pause_cause"),
        Some(&json!("unrecorded_screen")),
        "{pause:?}"
    );
    assert_eq!(
        pause.value("failure_code"),
        Some(&json!(UNRECORDED_SCREEN)),
        "{pause:?}"
    );
    host.close().expect("close runtime host");
}

// After it (two settled runs, two hours since the first), on a device instance: the same screen
// ends the run with `page_confirmation_failed` after one tap (its closing decision made, no
// further input), with none of the dedicated code's own values and with Workflow #469 K1's word
// `confirmation_problem` `unrecorded_screen`; the run's terminal and its recorded failure name a
// frame of its `Failure` pin as `artifact_id`; the failure is recoverable and scheduled for a
// retry by the recommended `on_failure`, and, once the ladder its class admits has ended (as
// gate_463_u1 waits for it), the pair is not paused.
#[test]
fn u1b_t2_after_the_break_in_the_same_screen_is_an_ordinary_failure_naming_its_frame() {
    let bench = Bench::new();
    let host = bench.host(true, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    bench.succeed(&host, T1, 8);
    let (context, error, taps) = bench.fail(&host, AfterTap::Unrecorded, T2, 9);
    assert_eq!(error.code(), CONFIRMATION_FAILED);
    assert_eq!(taps, 1, "the closing decision taps nothing");
    let failure = recorded_failure(&host, context.run_id());
    assert_eq!(failure.outcome.code().as_str(), CONFIRMATION_FAILED);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert_eq!(failure.original_class, PolicyFailureClass::Recoverable);
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(failure.outcome.value("page_ids"), None, "{failure:?}");
    assert_eq!(
        failure.outcome.value("confirmation_problem"),
        Some(&json!("unrecorded_screen")),
        "{failure:?}"
    );
    let named = failure
        .outcome
        .value("artifact_id")
        .cloned()
        .unwrap_or_else(|| panic!("the recorded failure names its frame: {failure:?}"));
    let frames = failure_frames(&host);
    assert!(frames.contains(&named), "{named} among {frames:?}");
    let terminals = failed_terminals(&host)
        .into_iter()
        .filter(|outcome| outcome.code().as_str() == CONFIRMATION_FAILED)
        .collect::<Vec<_>>();
    let [terminal] = terminals.as_slice() else {
        panic!("one failed terminal of the code: {terminals:?}");
    };
    assert_eq!(terminal.value("artifact_id").cloned(), Some(named));
    assert_eq!(
        terminal.value("confirmation_problem"),
        Some(&json!("unrecorded_screen")),
        "{terminal:?}"
    );
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let pause = pause_at(&host, &bench.clock, T3, 10);
    assert!(pause.is_none(), "no pause at once: {pause:?}");
    host.close().expect("close runtime host");
}

// Still inside with one of the two reached: two settled runs but two hours of three, and two hours
// of an hour and a half but two settled runs of three: the screen ends the run with the dedicated
// code and pauses its pair at once.
#[test]
fn u1b_t3_one_of_the_two_reached_is_still_inside() {
    for (case, runs, ms) in [
        ("runs reached, time not", RUNS, 3 * HOUR_MS),
        ("time reached, runs not", RUNS + 1, BREAK_IN_MS),
    ] {
        let bench = Bench::new();
        let host = bench.host(false, settings(Some((runs, ms))), true);
        bench.succeed(&host, T0, 7);
        bench.succeed(&host, T1, 8);
        let (context, error, _) = bench.fail(&host, AfterTap::Unrecorded, T2, 9);
        assert_eq!(error.code(), UNRECORDED_SCREEN, "{case}");
        assert_eq!(
            recorded_failure(&host, context.run_id()).disposition,
            PolicyFailureDisposition::PausedTask,
            "{case}"
        );
        host.close().expect("close runtime host");
    }
}

// A condition end and a stop count nothing: after one settled success, a scheduled run whose
// swallowed tap fails `page_confirmation_failed` and settles by a condition, and (on a device
// instance) a scheduled run stopped before its first step, leave the break-in at one run, its
// content and its first time as they were.
#[test]
fn u1b_t4_a_condition_end_and_a_stop_count_nothing() {
    let bench = Bench::new();
    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    let before = break_in(&host).expect("the break-in");
    host.settle_next_policy_run_by_condition_for_test(PolicyConditionSettlement {
        refunded: true,
        count: 1,
        expires_at_unix_ms: T0 + 10 * HOUR_MS,
    })
    .expect("settle the next run by a condition");
    let (context, error, _) = bench.fail(&host, AfterTap::OwnPage, T1, 8);
    assert_eq!(error.code(), CONFIRMATION_FAILED);
    assert!(
        recorded_failure(&host, context.run_id())
            .condition
            .is_some(),
        "the run ended by a condition"
    );
    assert_eq!(
        break_in(&host),
        Some(before),
        "a condition end counts nothing"
    );
    host.close().expect("close runtime host");

    let bench = Bench::new();
    let host = bench.host(true, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    let before = break_in(&host).expect("the break-in");
    show_after_next_tap(&bench.state, AfterTap::Reached);
    let context = bench.admitted(&host, T1, 8);
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let (sent, answered) = std::sync::mpsc::channel();
    let checkpoint = host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::PackageAdmitted,
            bench.registered,
            context.lease_token().lease_id(),
            move |identity| {
                sent.send(stop_run(address, identity.request_id()))
                    .expect("the stop's answer");
            },
        )
        .expect("arm the run's checkpoint");
    let error = host
        .run_scheduled_contained_task(&context, &bench.request)
        .expect_err("the stopped run does not complete");
    assert_eq!(checkpoint.consumed(), 1, "the run reached its checkpoint");
    let answer = answered.recv_timeout(GATE_WAIT).expect("the stop's answer");
    assert_eq!(answer.state(), RuntimeReceiptState::Completed, "{answer:?}");
    assert_eq!(error.code(), CANCELLED);
    assert_eq!(break_in(&host), Some(before), "a stop counts nothing");
    host.close().expect("close runtime host");
}

// A new content starts again: after two settled runs and two hours, the package changed in place
// (its taps 20 ms apart) meets the screen: its pair is in the break-in of the new content, so the
// run ends with the dedicated code and pauses its pair at once, and it is that break-in's first run.
#[test]
fn u1b_t5_a_new_content_starts_a_new_break_in() {
    let bench = Bench::new();
    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    bench.succeed(&host, T1, 8);
    let old = break_in(&host).expect("the break-in");
    assert_eq!(old.runs, 2);
    write_package(&bench.package, 20);
    let (context, error, _) = bench.fail(&host, AfterTap::Unrecorded, T2, 9);
    assert_eq!(error.code(), UNRECORDED_SCREEN);
    assert_eq!(
        recorded_failure(&host, context.run_id()).disposition,
        PolicyFailureDisposition::PausedTask
    );
    let new = break_in(&host).expect("the new content's break-in");
    assert_ne!(new.content_hash, old.content_hash);
    assert_eq!(new.runs, 1);
    assert!(new.first_settled_at_unix_ms >= T2, "{new:?}");
    host.close().expect("close runtime host");
}

// How this start of the host restored the decision memory, as its start report says (Workflow
// #422 LN-4): `Load` from the value records, or `Seed` (no marker: the dispatch records replayed).
fn tagged_start(host: &RuntimeHost) -> Option<StartReportTaggedState> {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("expected the start report: {receipt:?}");
    };
    report.tagged_state
}

// The host restarted between the first and the second settled run keeps the break-in as the first
// host wrote it, on both kinds of start (Workflow #422 LN-4, as gate_469_h2 asserts them): a
// root's first start writes no marker, so its second start seeds, replaying the dispatch records,
// which hold no break-in (fix round 2: it takes the pair's from its latest value record); its third
// start loads the value records, as every restart of an installation does. Each start's kind is
// asserted from its start report. After the second run, two hours after the first, the screen ends
// the run with `page_confirmation_failed` (a count or a first time lost at a restart would keep the
// pair inside).
#[test]
fn u1b_t6_a_restart_keeps_the_count_and_the_first_time() {
    let bench = Bench::new();
    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    let written = break_in(&host).expect("the break-in");
    host.close().expect("close runtime host");

    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), false);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Seed),
        "the root's second start seeds"
    );
    assert_eq!(
        break_in(&host),
        Some(written.clone()),
        "kept by the seeding start"
    );
    host.close().expect("close the seeding host");

    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), false);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Load),
        "the restart checked loads its decision memory from its value records"
    );
    assert_eq!(
        break_in(&host),
        Some(written.clone()),
        "loaded at the start"
    );
    bench.succeed(&host, T1, 8);
    let counted = break_in(&host).expect("the break-in");
    assert_eq!(counted.runs, 2);
    assert_eq!(
        counted.first_settled_at_unix_ms,
        written.first_settled_at_unix_ms
    );
    let (_, error, _) = bench.fail(&host, AfterTap::Unrecorded, T2, 9);
    assert_eq!(error.code(), CONFIRMATION_FAILED);
    host.close().expect("close runtime host");
}

// A direct run after the break-in: on a device instance whose pair is past its break-in (two
// settled runs, two hours), the package run as `actingctl task-run` runs it on the same screen ends
// Failed with the dedicated code, its terminal and receipt naming the frame of its `Failure` pin.
#[test]
fn u1b_t7_a_direct_run_after_the_break_in_keeps_the_dedicated_code() {
    let bench = Bench::new();
    let host = bench.host(true, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    bench.succeed(&host, T1, 8);
    advance_manual_clock_to(&bench.clock, T2);
    show_after_next_tap(&bench.state, AfterTap::Unrecorded);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(bench.package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(464).expect("connection"))
        .expect("run receipt");
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), UNRECORDED_SCREEN);
    let named = receipt
        .outcome()
        .value("artifact_id")
        .cloned()
        .unwrap_or_else(|| panic!("the receipt names its frame: {receipt:?}"));
    assert_eq!(failure_frames(&host), [named]);
    host.close().expect("close runtime host");
}

// The keys set while the Runtime runs count from the next run: with the defaults (20 runs, seven
// days, as the assembly renders them from the shipped defaults file, settable) the pair is inside
// after one settled run; `ConfigureSettings` then sets one run and one second, and the next run on
// the screen ends with `page_confirmation_failed`, not paused at once.
#[test]
fn u1b_t8_the_keys_set_while_running_count_from_the_next_run() {
    let defaults = rendered_break_in_defaults()
        .into_iter()
        .map(|row| (row.key, (row.value, row.source, row.settable)))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        defaults,
        std::collections::BTreeMap::from([
            (
                SUSPENSION_BREAK_IN_RUNS.to_owned(),
                (
                    FactScalar::Integer(20),
                    ConfigParameterSource::Default,
                    true
                )
            ),
            (
                SUSPENSION_BREAK_IN_MS.to_owned(),
                (
                    FactScalar::DurationMs(604_800_000),
                    ConfigParameterSource::Default,
                    true
                )
            ),
        ])
    );
    let bench = Bench::new();
    let host = bench.host(false, settings(None), true);
    bench.succeed(&host, T0, 7);
    assert_eq!(break_in(&host).map(|break_in| break_in.runs), Some(1));
    configure(&host, SUSPENSION_BREAK_IN_RUNS, json!(1));
    configure(&host, SUSPENSION_BREAK_IN_MS, json!(1_000));
    let (context, error, _) = bench.fail(&host, AfterTap::Unrecorded, T1, 8);
    assert_eq!(error.code(), CONFIRMATION_FAILED);
    assert_eq!(
        recorded_failure(&host, context.run_id()).disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    host.close().expect("close runtime host");
}

// ---- Fix round 3 (`rd-1115/brief-463-u1b-fix3.md`, C 5).

// The task facts the host recorded for the run `run_id`.
fn run_facts(host: &RuntimeHost, run_id: RunId) -> Vec<TaskSemanticFact> {
    all_events(host)
        .iter()
        .filter(|event| event.links().run_id() == Some(&run_id))
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact().clone()),
            _ => None,
        })
        .collect()
}

// The `policy_break_in_unread` warnings (Warning `runtime.failed`) recorded for the run `run_id`.
fn break_in_unread_warnings(host: &RuntimeHost, run_id: RunId) -> Vec<Outcome> {
    all_events(host)
        .iter()
        .filter(|event| event.links().run_id() == Some(&run_id))
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(actingcommand_contract::RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == BREAK_IN_UNREAD =>
            {
                Some(record.outcome().clone())
            }
            _ => None,
        })
        .collect()
}

// A run that never began its package counts nothing toward the break-in: the bench's package
// declares the prerequisite package `lead`, which the game root of `neutral` lists. After one
// settled success (on `home` the gate runs no layer and the package runs), a scheduled run on a
// screen neither package knows: the gate runs `lead`, which fails (its `EntryRecoveryFailed`, role
// `prerequisite`), so the run's own package never began (no `PackageAdmitted` in the run, no
// input); the run fails without poisoning the Runtime and the break-in is as it was: one run, its
// content, its first time. (The other example of the work order, a refusal of a handset's start
// gate, needs a wired handset on a scheduled run, which this gate's fixtures do not have; the rule
// is the same flag, `PackageAdmitted`, which the gate's check runs before.)
#[test]
fn u1b_t9_a_run_whose_prerequisite_layer_failed_counts_nothing() {
    let mut bench = Bench::new();
    write_package_with(&bench.package, 10, Some(LEAD));
    let mut standard = super::gate_373_nav3a::StandardRoot::new(bench.root.path());
    write_lead(&standard.list(LEAD));
    standard.write_bundle();
    bench.standard_packs = Some(standard.root.clone());
    let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
    bench.succeed(&host, T0, 7);
    let before = break_in(&host).expect("the success starts the break-in");
    assert_eq!(before.runs, 1);
    show_unknown_from_now(&bench.state);
    let (context, _, taps) = bench.fail_as_shown(&host, T1, 8);
    assert_eq!(taps, 0, "no input");
    let facts = run_facts(&host, context.run_id());
    let failed_layers = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskSemanticFact::EntryRecoveryFailed {
                package_label,
                role,
                ..
            } => Some((package_label.clone(), *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failed_layers,
        [(
            Some(LEAD.to_owned()),
            Some(TaskEntryRecoveryRole::Prerequisite)
        )],
        "{facts:?}"
    );
    assert!(
        !facts
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::PackageAdmitted { .. })),
        "the run's own package never began: {facts:?}"
    );
    assert_eq!(
        break_in(&host),
        Some(before),
        "a run that never began its package counts nothing"
    );
    host.close().expect("close runtime host");
}

// A break-in that cannot be established: after one settled success, a scheduled run on the screen
// the package never recorded whose preparation cannot measure the package's content
// (`content_unmeasured`), and on another bench one whose read of the pair's break-in fails as a
// dispatch the decision memory does not hold fails it (`record_unread`). The failure is made once
// at that read (`fail_next_break_in_read_for_test`), as gate_463_u1 makes the read of a run's
// budget end fail: no fixture makes these reads fail otherwise, since the measurement reads the
// package the preparation has just admitted and the decision memory holds every admitted
// dispatch. Each run records one `policy_break_in_unread` with its word, its task and its instance,
// goes on as one outside its break-in (`page_confirmation_failed` after one tap, retry scheduled,
// so not paused at once, though one run of two is inside) and counts by the rule: a run whose
// content was not measured counts nothing; a run whose break-in could not be read began its
// measured content and counts.
#[test]
fn u1b_t10_a_break_in_that_cannot_be_established_warns_and_the_run_goes_on() {
    for (problem, counted) in [
        (BreakInProblem::ContentUnmeasured, 1),
        (BreakInProblem::RecordUnread, 2),
    ] {
        let bench = Bench::new();
        let host = bench.host(false, settings(Some((RUNS, BREAK_IN_MS))), true);
        bench.succeed(&host, T0, 7);
        let before = break_in(&host).expect("the break-in");
        host.fail_next_break_in_read_for_test(problem)
            .expect("arm the failed read");
        let (context, error, taps) = bench.fail(&host, AfterTap::Unrecorded, T1, 8);
        assert_eq!(error.code(), CONFIRMATION_FAILED, "{problem:?}");
        assert_eq!(taps, 1, "{problem:?}");
        assert_eq!(
            recorded_failure(&host, context.run_id()).disposition,
            PolicyFailureDisposition::RetryScheduled,
            "{problem:?}"
        );
        let warnings = break_in_unread_warnings(&host, context.run_id());
        let [warning] = warnings.as_slice() else {
            panic!("{problem:?}: one warning: {warnings:?}");
        };
        assert_eq!(
            warning.value("break_in_problem"),
            Some(&json!(problem.as_str())),
            "{warning:?}"
        );
        assert_eq!(warning.value("task_id"), Some(&json!(TASK)), "{warning:?}");
        assert_eq!(
            warning.value("instance_id"),
            Some(&json!(POLICY_INSTANCE_ALIAS)),
            "{warning:?}"
        );
        let after = break_in(&host).expect("the break-in");
        assert_eq!(after.runs, counted, "{problem:?}");
        assert_eq!(after.content_hash, before.content_hash, "{problem:?}");
        assert_eq!(
            after.first_settled_at_unix_ms, before.first_settled_at_unix_ms,
            "{problem:?}"
        );
        host.close().expect("close runtime host");
    }
}

// `actingd suspended` on the bench's closed root, judged by `settings(Some((RUNS, ms_needed)))`:
// its one `break_ins` row.
fn break_in_row(bench: &Bench, ms_needed: u64) -> Value {
    let report = crate::suspension_report(&SuspensionReportRequest {
        config_path: Path::new("actingd.config.json"),
        config_modified_unix_ms: None,
        host: &config(&bench.root)
            .with_procedure_manifest(procedures_at(&bench.package))
            .with_config_manifest(settings(Some((RUNS, ms_needed)))),
        catalog: &sources(),
    })
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"));
    let rows = report["break_ins"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the break_ins rows: {report:#}"));
    let [row] = rows.as_slice() else {
        panic!("one break_ins row: {report:#}");
    };
    row.clone()
}

// `actingd suspended` lists the pair's break-in row (`break_ins`), judged by the configuration the
// report loads (two runs, one second): after the first settled run, one run of two and
// `in_break_in` true; after a restart (the root's second start, which seeds, as its start report
// says) and a second settled run an hour later, two runs of two, more than a second since the
// first and `in_break_in` false. Each row carries the content and the first time the decision
// memory held, the two needed values, and `since_first_ms`, the report's now (the ledger's head
// row) less the first time: after the second run that now is no earlier than the second run.
#[test]
fn u1b_t11_actingd_suspended_lists_the_break_in_before_and_after_it_ends() {
    const MS_NEEDED: u64 = 1_000;
    let bench = Bench::new();
    let host = bench.host(false, settings(Some((RUNS, MS_NEEDED))), true);
    bench.succeed(&host, T0, 7);
    let first = break_in(&host).expect("the break-in");
    host.close().expect("close runtime host");
    let row = break_in_row(&bench, MS_NEEDED);
    let since = row["since_first_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("since_first_ms: {row:#}"));
    assert_eq!(
        row,
        json!({
            "task_id": TASK,
            "instance_id": POLICY_INSTANCE_ALIAS,
            "content_hash_prefix": first.content_hash,
            "runs": 1,
            "runs_needed": RUNS,
            "first_settled_at_unix_ms": first.first_settled_at_unix_ms,
            "since_first_ms": since,
            "ms_needed": MS_NEEDED,
            "in_break_in": true,
        }),
        "before the break-in ends"
    );

    let host = bench.host(false, settings(Some((RUNS, MS_NEEDED))), false);
    assert_eq!(
        tagged_start(&host),
        Some(StartReportTaggedState::Seed),
        "the root's second start seeds"
    );
    bench.succeed(&host, T1, 8);
    host.close().expect("close the restarted host");
    let row = break_in_row(&bench, MS_NEEDED);
    let since = row["since_first_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("since_first_ms: {row:#}"));
    assert!(
        since >= MS_NEEDED && first.first_settled_at_unix_ms + since >= T1,
        "{row:#}"
    );
    assert_eq!(
        row,
        json!({
            "task_id": TASK,
            "instance_id": POLICY_INSTANCE_ALIAS,
            "content_hash_prefix": first.content_hash,
            "runs": 2,
            "runs_needed": RUNS,
            "first_settled_at_unix_ms": first.first_settled_at_unix_ms,
            "since_first_ms": since,
            "ms_needed": MS_NEEDED,
            "in_break_in": false,
        }),
        "after the break-in ends"
    );
}
