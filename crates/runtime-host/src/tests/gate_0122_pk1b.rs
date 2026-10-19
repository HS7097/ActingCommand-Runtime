// SPDX-License-Identifier: AGPL-3.0-only

// #385 P-k1b (v0.12.x; host gate 110; register §1.107; PERF v1.4.2 §4.6, §5.3, §5.6, §10.3, §12;
// test rows Tk-5, Tk-10, Tk-13, Tm-7; Alice 2026-10-19, pending item 4): the host hands each
// step its instance's pacing factor in effect and records it, keeps its own deadline on the
// wall clock, and settles a stretched policy run on its budget time. Every expected value is the
// brief's:
//
// - the factor in effect is the curve's (pressure floor included) under `pace`, 1 000 under
//   `observe` and before the first evaluation (requirement 1);
// - a step's waits last the package's values × f / 1000, and a step above 1 000 has one
//   `steps[]` entry with its factor; at 1 000 nothing is recorded (requirements 1, 5, 6);
// - the host's deadline and its `contained_task_deadline_exceeded` do not move (requirement 2);
// - a policy run is compared with its runtime budget on its budget time, wall time less the
//   paced part's wall × (f − 1000) / f, and its record carries both (requirements 3, 4, 6).
//
// The factor of tests 2 to 4 is pinned (`pin_pacing_factor_for_test`); test 1 reads the shadow
// measurement itself. Runs wait on `WaitClock`, which moves only when the kernel waits and keeps
// each wait, so every time below is the Runtime clock's; nothing here sleeps or reads a clock.
// The shipped curve: (10 000, S0, 1 000), (9 000, S0, 1 000), (7 500, S1, 1 500),
// (6 000, S2, 2 500), (4 500, S3, 3 250), (3 000, S3, 4 000), (0, S3, 4 000), linear; the
// pressure floor gives at least 1 250. A frame acquisition's allowance on a backend the
// configuration does not name is 55 000 us, so 91 666 us scores 550 000 000 / 91 666 = 6 000.

use super::*;
use crate::codes::ShapeHostCode;
use crate::performance::{PacingSample, PacingSampleSource, PacingShadow};
use actingcommand_contract::{
    ContainedTaskCancellationReason, PerformancePacingMode, PerformancePressureSeverity,
};
use actingcommand_runtime_config::EffectiveConfig;
use actingcommand_runtime_config::performance::PerformanceMeasurement;
use serde_json::json;
use std::sync::Mutex;

const ALIAS: &str = "neutral.instance";
// The shadow measurement's evaluations: a first one, then one degrade sustain (4 s) later.
const T0: u64 = 1_760_000_000_000;
const SUSTAINED: u64 = T0 + 4_000;
// A frame acquisition that scores 6 000, where the shipped curve gives S2 and 2 500.
const CURVE_2500_CAPTURE_US: u64 = 91_666;
// The pages' first pixel.
const HOME: [u8; 3] = [255, 0, 0];
const PAGE_A: [u8; 3] = [0, 255, 255];
const TERMINAL: [u8; 3] = [0, 0, 255];
// Test 4's task: its admitted runtime budget and its expected duration.
const RUNTIME_BUDGET_MS: u64 = 10_000;
const EXPECTED_DURATION_MS: u64 = 1_000;

/// A Runtime clock that moves only when the contained-task kernel waits, by the wait's length
/// rounded up to the millisecond (test plan E-3, §3.10), and keeps every wait; a test may also
/// move it on itself. It never reads the process clock.
struct WaitClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
    waits: Mutex<Vec<Duration>>,
}

impl WaitClock {
    fn new(unix_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
            waits: Mutex::new(Vec::new()),
        }
    }

    fn advance(&self, duration_ms: u64) {
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }

    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(Ordering::SeqCst)
    }

    /// Every wait so far, in milliseconds, in order.
    fn waits_ms(&self) -> Vec<u64> {
        self.waits
            .lock()
            .expect("the clock's waits")
            .iter()
            .map(|wait| u64::try_from(wait.as_millis()).expect("a wait in milliseconds"))
            .collect()
    }
}

impl RuntimeClock for WaitClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.load(Ordering::SeqCst),
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }

    fn pause(&self, duration: Duration) {
        self.waits.lock().expect("the clock's waits").push(duration);
        self.advance(u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX));
    }
}

fn shipped() -> PerformanceMeasurement {
    EffectiveConfig::default()
        .performance_measurement()
        .expect("the shipped performance section")
}

/// The shadow measurement after ten frames of `ALIAS` that score 6 000 and two evaluations one
/// degrade sustain apart under `settings`.
fn evaluated_at_6000(settings: &PerformanceMeasurement) -> PacingShadow {
    let instances = vec![ALIAS.to_owned()];
    let mut shadow = PacingShadow::default();
    for index in 0..10 {
        shadow.record(PacingSample {
            observed_at_unix_ms: T0 - 1_000 + index,
            source: PacingSampleSource::CaptureAcquire {
                instance_alias: ALIAS.to_owned(),
            },
            value_us: CURVE_2500_CAPTURE_US,
        });
    }
    shadow.evaluate(T0, settings, &instances, None);
    shadow.evaluate(SUSTAINED, settings, &instances, None);
    shadow
}

/// A page-graph package over `home`, `page_a` and `terminal` (told apart by the frame's first
/// pixel) from `home` to `terminal`: each operation `(id, from, to, post_delay_ms)` taps once and
/// waits for its `to` page.
fn package(
    operations: &[(&str, &str, &str, Option<u64>)],
    capture_interval_ms: u64,
    step_timeout_ms: u64,
    timeout_ms: u64,
) -> Vec<u8> {
    let operations = operations
        .iter()
        .map(|(id, from, to, post_delay_ms)| {
            let mut operation = json!({
                "id": id,
                "from": from,
                "to": to,
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            });
            if let Some(delay) = post_delay_ms {
                operation["post_delay_ms"] = json!(delay);
            }
            operation
        })
        .collect::<Vec<_>>();
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": operations
    });
    let target = |id: &str, expected: [u8; 3]| {
        json!({
            "type": "color",
            "id": id,
            "region": {"x": 0, "y": 0, "width": 1, "height": 1},
            "expected": expected
        })
    };
    let recognition = json!({
        "schema_version": "0.3",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            target("page/home", HOME),
            target("page/a", PAGE_A),
            target("page/terminal", TERMINAL)
        ]
    });
    let page = |id: &str, required: &str| {
        json!({
            "id": id,
            "required": [required],
            "optional": [],
            "forbidden": []
        })
    };
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            page("neutral/home", "page/home"),
            page("neutral/page_a", "page/a"),
            page("neutral/terminal", "page/terminal")
        ]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.pk1b.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": capture_interval_ms,
        "step_timeout_ms": step_timeout_ms,
        "timeout_ms": timeout_ms,
        "max_steps": 4
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The two-step package of tests 2 and 4: `home` to `page_a` with `post_delay_ms` `first`, then
/// `page_a` to `terminal` with `second`, a frame every millisecond.
fn two_step_package(first: u64, second: u64) -> Vec<u8> {
    package(
        &[
            ("open_a", "home", "page_a", Some(first)),
            ("open_terminal", "page_a", "terminal", Some(second)),
        ],
        1,
        5_000,
        60_000,
    )
}

/// The device shows `home` before the first tap, `page_a` after it and `terminal` after the
/// second.
fn three_pages(state: &FakeState) {
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            vec![(
                0,
                match inputs {
                    0 => HOME,
                    1 => PAGE_A,
                    _ => TERMINAL,
                },
            )]
        },
    ));
}

/// A host whose one physical instance `ALIAS` shows a 2x1 frame (Workflow #392 DA-A: a direct
/// run needs an observed display), on `clock`.
fn direct_host(root: &TempDir, clock: &Arc<WaitClock>, state: &Arc<FakeState>) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    RuntimeHost::start(
        config(root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host")
}

/// One direct run of `package` on `ALIAS`: its receipt and its request's records.
fn run_direct(
    host: &RuntimeHost,
    package: &Path,
    response_deadline_ms: u64,
) -> (RuntimeReceipt, Vec<PersistedEvent>) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string())
                .expect("contained task request")
                .with_response_deadline_ms(response_deadline_ms)
                .expect("bounded response deadline"),
        ),
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(110).expect("connection"))
        .expect("run receipt");
    let events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        })
        .expect("run records");
    (receipt, events)
}

/// The `steps[]` of the run's task timing evidence, carried by its one terminal:
/// `(step_index, pacing_factor_milli)` each.
fn step_factors(events: &[PersistedEvent]) -> Vec<(u32, Option<u32>)> {
    let timings = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
            )
        })
        .filter_map(|event| event.payload().task_timing())
        .collect::<Vec<_>>();
    let [timing] = timings.as_slice() else {
        panic!("one terminal with its task timing evidence: {timings:?}");
    };
    timing
        .steps
        .iter()
        .map(|step| (step.step_index, step.pacing_factor_milli))
        .collect()
}

/// The host deadline the run admitted (`task.package_admitted`).
fn admitted_deadline(events: &[PersistedEvent]) -> Option<u64> {
    events.iter().find_map(|event| match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::PackageAdmitted {
                response_deadline_monotonic_ms,
                ..
            } => *response_deadline_monotonic_ms,
            _ => None,
        },
        _ => None,
    })
}

/// The fixture catalog whose task expects `EXPECTED_DURATION_MS` and may run
/// `RUNTIME_BUDGET_MS` per window.
fn budget_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    tasks["tasks"][0]["expected_duration_ms"] = json!(EXPECTED_DURATION_MS);
    tasks["tasks"][0]["loop_budget"]["max_runtime_ms"] = json!(RUNTIME_BUDGET_MS);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture task bytes");
    sources
}

// Test 1 (requirement 1): the factor a step is handed is its instance's factor in effect at the
// shadow measurement's last evaluation: under `pace` the curve's (2 500 at 6 000), under
// `observe` 1 000 on the same inputs, 1 000 before any evaluation, and under `pace` with a
// pressure of a floor kind at full responsiveness the floor's factor (1 250).
#[test]
fn pk1b_the_step_factor_is_the_instances_factor_in_effect() {
    let observe = shipped();
    assert_eq!(observe.pacing.mode, PerformancePacingMode::Observe);
    let mut pace = shipped();
    pace.pacing.mode = PerformancePacingMode::Pace;

    assert_eq!(
        PacingShadow::default().effective_factor_milli(ALIAS),
        1_000,
        "no evaluation yet"
    );
    assert_eq!(
        evaluated_at_6000(&pace).effective_factor_milli(ALIAS),
        2_500,
        "pace"
    );
    assert_eq!(
        evaluated_at_6000(&observe).effective_factor_milli(ALIAS),
        1_000,
        "observe"
    );

    let mut pressured = PacingShadow::default();
    pressured.evaluate(
        T0,
        &pace,
        &[ALIAS.to_owned()],
        Some(PerformancePressureSeverity::Elevated),
    );
    assert_eq!(pace.pacing.pressure_floor.factor_milli, 1_250);
    assert_eq!(
        pressured.effective_factor_milli(ALIAS),
        pace.pacing.pressure_floor.factor_milli,
        "the pressure floor under pace"
    );
}

// Test 2 (requirements 1, 5, 6): a direct run at a pinned 2 000 waits twice each step's
// post-input delay (300 ms and 700 ms) and its evidence has one `steps[]` entry per step with
// `pacing_factor_milli` 2 000; at 1 000 it waits the package's values and has no `steps[]`. The
// waits of 100 ms or more are the post-input delays: every other wait of these runs is a frame
// interval of 1 ms × f.
#[test]
fn pk1b_a_run_hands_each_step_its_factor_and_records_it() {
    for (factor_milli, delays_ms, steps) in [
        (
            2_000,
            vec![600, 1_400],
            vec![(0, Some(2_000)), (1, Some(2_000))],
        ),
        (1_000, vec![300, 700], Vec::new()),
    ] {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("two-steps.zip");
        fs::write(&path, two_step_package(300, 700)).expect("write package");
        let state = Arc::new(FakeState::default());
        three_pages(&state);
        let clock = Arc::new(WaitClock::new(1_000));
        let host = direct_host(&root, &clock, &state);
        host.pin_pacing_factor_for_test(ALIAS, factor_milli)
            .expect("pin the factor");

        let (receipt, events) = run_direct(&host, &path, 60_000);
        host.close().expect("close runtime host");

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "factor {factor_milli}: {receipt:?}"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 2);
        let post_input_waits = clock
            .waits_ms()
            .into_iter()
            .filter(|wait| *wait >= 100)
            .collect::<Vec<_>>();
        assert_eq!(post_input_waits, delays_ms, "factor {factor_milli}");
        assert_eq!(step_factors(&events), steps, "factor {factor_milli}");
    }
}

// Test 3 (requirement 2; Tk-13): a direct run with `response_deadline_ms` 60 000, a 50 s task
// timeout and a target page that never arrives, at a pinned 2 000, ends
// `contained_task_deadline_exceeded` (cancelled, reason `deadline_exceeded`) when the Runtime
// clock reaches its host deadline, within one wait of it; the deadline it admitted is the one a
// run at 1 000 admits, and no `perf_run_yielded` is written. At 2 000 a frame interval of 5 s is
// a wait of 10 s.
#[test]
fn pk1b_the_host_deadline_does_not_move_under_a_factor() {
    const RESPONSE_DEADLINE_MS: u64 = 60_000;
    const PACED_WAIT_MS: u64 = 10_000;
    let never_arrives = package(
        &[("open_terminal", "home", "terminal", None)],
        5_000,
        50_000,
        50_000,
    );
    let mut deadlines = Vec::new();
    for factor_milli in [2_000, 1_000] {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("never-arrives.zip");
        fs::write(&path, &never_arrives).expect("write package");
        // The device stays on `home` after the tap.
        let state = Arc::new(FakeState::default());
        let clock = Arc::new(WaitClock::new(1_000));
        let host = direct_host(&root, &clock, &state);
        host.pin_pacing_factor_for_test(ALIAS, factor_milli)
            .expect("pin the factor");

        let (receipt, events) = run_direct(&host, &path, RESPONSE_DEADLINE_MS);
        let all_records = host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("every record");
        host.close().expect("close runtime host");

        deadlines.push(admitted_deadline(&events));
        if factor_milli == 2_000 {
            assert_eq!(
                receipt.state(),
                RuntimeReceiptState::Cancelled,
                "{receipt:#?}"
            );
            assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::ContainedTaskCancelled {
                        reason: ContainedTaskCancellationReason::DeadlineExceeded,
                        ..
                    })
                ),
                "{:?}",
                receipt.result()
            );
            let ended_ms = clock.monotonic_ms();
            assert!(
                (RESPONSE_DEADLINE_MS..RESPONSE_DEADLINE_MS + PACED_WAIT_MS).contains(&ended_ms),
                "the run ended at {ended_ms} ms"
            );
            let yielded = ShapeHostCode::PerfRunYielded.as_str();
            assert!(
                !all_records.iter().any(|event| {
                    serde_json::to_string(event.payload())
                        .expect("payload JSON")
                        .contains(yielded)
                }),
                "no {yielded} record"
            );
        }
    }
    assert_eq!(
        deadlines,
        vec![Some(RESPONSE_DEADLINE_MS), Some(RESPONSE_DEADLINE_MS)],
        "the host deadline at 2 000 and at 1 000"
    );
}

// Test 4 (requirements 3, 4, 6; Tk-10): a scheduled run at a pinned 2 000 whose wall time (two
// post-input delays of 3 s, waited 6 s each: 12 s) is over its task's 10 s runtime budget and
// whose budget time is not (12 s − 12 s / 2 = 6 s) succeeds without a rewrite; its record carries
// the wall time and the budget time, wall − the paced part's wall / 2. The same 12 s of wall time
// at 1 000 (the 6 s of delays and 6 s before the run) is rewritten to
// `policy_runtime_budget_exceeded`, and its record has neither member.
#[test]
fn pk1b_a_stretched_policy_run_is_settled_on_its_budget_time() {
    const DELAY_MS: u64 = 3_000;
    let package = two_step_package(DELAY_MS, DELAY_MS);
    for (factor_milli, before_run_ms) in [(2_000, 0), (1_000, 2 * DELAY_MS)] {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("scheduled-two-steps.zip");
        fs::write(&path, &package).expect("write package");
        let state = Arc::new(FakeState::default());
        three_pages(&state);
        let clock = Arc::new(WaitClock::new(POLICY_NOW_UNIX_MS));
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
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
        host.activate_policy_catalog(&budget_sources())
            .expect("activate the policy catalog");
        host.pin_pacing_factor_for_test(POLICY_INSTANCE_ALIAS, factor_milli)
            .expect("pin the factor");
        let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
        record_policy_approval(&host, &intent);
        let request = ContainedTaskRequest::new(path.to_string_lossy().into_owned())
            .expect("contained task request");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&host, &intent),
                &request,
            )
            .expect("policy admission")
        else {
            panic!("expected one policy run context")
        };
        let admitted_ms = clock.monotonic_ms();
        clock.advance(before_run_ms);
        let receipt = host
            .run_scheduled_contained_task(&context, &request)
            .expect("the scheduled run");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "factor {factor_milli}: {receipt:?}"
        );
        host.complete_scheduled_policy_run(&context, &receipt)
            .expect("the run settles");
        let wall_ms = clock.monotonic_ms() - admitted_ms;
        let paced_ms: u64 = if factor_milli == 2_000 {
            clock.waits_ms().iter().sum()
        } else {
            0
        };
        let records = host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            })
            .expect("the run's records")
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => Some((
                    payload.policy_result().clone(),
                    payload.wall_elapsed_ms(),
                    payload.budget_elapsed_ms(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        host.close().expect("close runtime host");

        assert_eq!(wall_ms, 4 * DELAY_MS, "factor {factor_milli}");
        assert!(wall_ms > RUNTIME_BUDGET_MS);
        let [(result, wall_elapsed_ms, budget_elapsed_ms)] = records.as_slice() else {
            panic!("one execution record: {records:?}");
        };
        if factor_milli == 2_000 {
            assert_eq!(paced_ms, wall_ms, "every wait of the run is paced");
            let budget_ms = wall_ms - paced_ms / 2;
            assert!(budget_ms <= RUNTIME_BUDGET_MS);
            assert!(
                matches!(result, PolicyExecutionOutcome::Succeeded { .. }),
                "no rewrite: {result:?}"
            );
            assert_eq!(
                (*wall_elapsed_ms, *budget_elapsed_ms),
                (Some(wall_ms), Some(budget_ms))
            );
        } else {
            let PolicyExecutionOutcome::Failed { failure } = result else {
                panic!("the rewrite: {result:?}");
            };
            assert!(
                failure
                    .outcome
                    .is(actingcommand_contract::POLICY_RUNTIME_BUDGET_EXCEEDED),
                "{failure:?}"
            );
            assert_eq!(failure.runtime_ms, wall_ms);
            assert_eq!((*wall_elapsed_ms, *budget_elapsed_ms), (None, None));
        }
    }
}
