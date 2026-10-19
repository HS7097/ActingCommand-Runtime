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
use std::sync::{Mutex, OnceLock};

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

/// Samples business capacity once and records it, as the capacity monitor's tick does
/// (`RuntimeHost::capacity_sampler_for_test`).
type CapacitySampler = Box<dyn Fn() -> RuntimeHostResult<()> + Send>;

/// A Runtime clock that moves only when the contained-task kernel waits, by the wait's length
/// rounded up to the millisecond (test plan E-3, §3.10), and keeps every wait; a test may also
/// move it on itself. It never reads the process clock.
///
/// Every move samples business capacity once it has moved the clock. A capacity sample is fresh
/// for twice the monitor's interval on the Runtime clock (2 x 2 s; contracts/capacity-admission.md
/// "capacity freshness"), and the monitor samples on its own interval of real time, but a wait on
/// this clock moves it by up to 10 s at once: without a sample at the moved time the run's next
/// frame is refused `capacity_admission_refused` at `admit_artifact_bytes` (CI run 39203831479),
/// a scene the requirements do not describe. So each move samples capacity, as the monitor does
/// while time passes: the fix of `gate_382_h4.rs` `CapacitySamplingClock`, whose deadlock
/// argument holds here unchanged (this clock's `sample` takes no lock, and its waits lock is
/// released before the sampler runs).
struct WaitClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
    waits: Mutex<Vec<Duration>>,
    sampler: OnceLock<Mutex<CapacitySampler>>,
}

impl WaitClock {
    fn new(unix_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
            waits: Mutex::new(Vec::new()),
            sampler: OnceLock::new(),
        }
    }

    /// Every later move samples capacity with `sampler`; set once, right after the host starts,
    /// before any run.
    fn sample_capacity_after_moves(&self, sampler: CapacitySampler) {
        assert!(
            self.sampler.set(Mutex::new(sampler)).is_ok(),
            "the capacity sampler is set once"
        );
    }

    fn advance(&self, duration_ms: u64) {
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
        if let Some(sampler) = self.sampler.get() {
            let sample_capacity = sampler.lock().expect("the capacity sampler's lock");
            (*sample_capacity)().expect("capacity sample at the time the clock moved to");
        }
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
/// run needs an observed display), on `clock`, which samples capacity after each move.
fn direct_host(root: &TempDir, clock: &Arc<WaitClock>, state: &Arc<FakeState>) -> RuntimeHost {
    direct_host_with(config(root), clock, state)
}

/// [`direct_host`] on `config`.
fn direct_host_with(
    config: RuntimeHostConfig,
    clock: &Arc<WaitClock>,
    state: &Arc<FakeState>,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let host = RuntimeHost::start(
        config.with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(state))),
    )
    .expect("runtime host");
    clock.sample_capacity_after_moves(host.capacity_sampler_for_test().expect("capacity owner"));
    host
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
// waits of 100 ms or more are the post-input delays and (#385 P-k2, PK2 requirement 5) at 2 000
// each step's pre-input wait before its tap, the shipped base 300 ms × (2 000 − 1 000) / 1 000
// = 300 ms: every other wait of these runs is a frame interval of 1 ms × f.
#[test]
fn pk1b_a_run_hands_each_step_its_factor_and_records_it() {
    for (factor_milli, delays_ms, steps) in [
        (
            2_000,
            vec![300, 600, 300, 1_400],
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
        // The shipped scheduler (`SchedulerConfig::default()`; no configuration file member sets
        // it). A direct run's lease ends one heartbeat reserve after its host deadline
        // (`contained_task_lease_ttl`, `contained_task_deadline_until`), and the run sees its
        // deadline at its first check after it, here after a paced wait. The fixture scheduler's
        // reserve of 20 ms (`config`) ended the lease at 60 020 ms, before a run that ends at
        // 60 300 ms (#385 P-k2: the step's 300 ms pre-input wait at 2 000 comes first) writes
        // its terminal; the lease sweep, every 50 ms of real time, then took the lapsed lease
        // and the terminal found none (`lease_mismatch`, CI run 39232870530), a scene the
        // requirement does not describe.
        let host = direct_host_with(
            config(&root).with_scheduler(SchedulerConfig::default()),
            &clock,
            &state,
        );
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
// post-input delays of 3 s, waited 6 s each, and, #385 P-k2, a 300 ms pre-input wait before each
// of its two taps: 12.6 s) is over its task's 10 s runtime budget and whose budget time is not
// (12.6 s − 12.6 s / 2 = 6.3 s) succeeds without a rewrite; its record carries the wall time and
// the budget time, wall − the paced part's wall / 2. 12 s of wall time at 1 000 (the 6 s of
// delays and 6 s before the run; no pre-input wait at 1 000) is rewritten to
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
        clock
            .sample_capacity_after_moves(host.capacity_sampler_for_test().expect("capacity owner"));
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

        // #385 P-k2: the two pre-input waits of 300 ms at 2 000.
        let pre_input_ms = if factor_milli == 2_000 { 2 * 300 } else { 0 };
        assert_eq!(
            wall_ms,
            4 * DELAY_MS + pre_input_ms,
            "factor {factor_milli}"
        );
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

// #385 P-k2 (host gate 111; register §1.108; PERF v1.4.2 §4.1, §4.4, §5.2, §5.4; brief PK2
// requirements 1, 2 and 4): what a step is handed beside its factor, and the rounds it used.

/// The `steps[]` of the run's task timing evidence, carried by its one terminal:
/// `(step_index, pacing_factor_milli, extra_wait_rounds)` each.
fn step_entries(events: &[PersistedEvent]) -> Vec<(u32, Option<u32>, Option<u32>)> {
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
        .map(|step| {
            (
                step.step_index,
                step.pacing_factor_milli,
                step.extra_wait_rounds,
            )
        })
        .collect()
}

// Test 5 (requirements 1, 2): the step pacing a step is handed, from its instance's reading at the
// shadow measurement's last evaluation and the configured bases. Under `pace` at 6 000 the default
// curve reads S2 (factor 2 500, extra 2): 2 extra wait rounds, the configured observe-only retries
// plus 2, and a pre-input wait of 300 ms × (2 500 − 1 000) / 1 000 = 450 ms. Under `observe` on the
// same inputs: no round, the configured retries, no pre-input wait. Under `pace` with only a
// pressure of a floor kind at full responsiveness: at least the floor's one round.
#[test]
fn pk2_the_step_pacing_carries_the_stages_extra_rounds_and_pre_input_wait() {
    let observe = shipped();
    let mut pace = shipped();
    pace.pacing.mode = PerformancePacingMode::Pace;
    let base = EffectiveConfig::default()
        .sample_timeout_retries()
        .expect("the shipped observe-only retries");
    let pre_input_wait_base_ms = pace.pacing.pre_input_wait_base_ms;
    assert_eq!(pre_input_wait_base_ms, 300, "the shipped pre-input base");
    let handed = |pacing: actingcommand_execution_kernel::StepPacing| {
        (
            pacing.factor_milli,
            pacing.extra_wait_rounds,
            pacing.sample_timeout_retries,
            pacing.pre_input_wait,
        )
    };

    let paced = evaluated_at_6000(&pace).effective_pacing(ALIAS);
    assert_eq!(
        handed(paced.step_pacing(base, pre_input_wait_base_ms)),
        (2_500, 2, base + 2, Duration::from_millis(450)),
        "pace at S2"
    );
    let observed = evaluated_at_6000(&observe).effective_pacing(ALIAS);
    assert_eq!(
        handed(observed.step_pacing(base, pre_input_wait_base_ms)),
        (1_000, 0, base, Duration::ZERO),
        "observe"
    );

    let mut pressured = PacingShadow::default();
    pressured.evaluate(
        T0,
        &pace,
        &[ALIAS.to_owned()],
        Some(PerformancePressureSeverity::Elevated),
    );
    assert_eq!(pace.pacing.pressure_floor.extra_retries, 1);
    let floor = pressured
        .effective_pacing(ALIAS)
        .step_pacing(base, pre_input_wait_base_ms);
    assert!(
        floor.extra_wait_rounds >= pace.pacing.pressure_floor.extra_retries,
        "the pressure floor's rounds: {floor:?}"
    );
    assert_eq!(floor.sample_timeout_retries, base + floor.extra_wait_rounds);
}

// Test 6 (requirement 4): a direct run of one tap from `home` to `terminal`, its confirmation a
// frame every 1 s for 3 s (the package's capture interval and step timeout). At a pinned 2 000
// with one extra round the device shows `terminal` from the sixth frame after the tap on: the
// first round's frames (0, 1, 2 and 3 s) show `home`, and the second round's second frame
// confirms the step, so the step's `steps[]` entry has `extra_wait_rounds` 1 beside its factor.
// At 1 000 with no extra round and `terminal` on the first frame after the tap the run uses none
// and has no `steps[]` entry.
#[test]
fn pk2_a_steps_used_rounds_are_recorded() {
    let one_tap = package(
        &[("open_terminal", "home", "terminal", None)],
        1_000,
        3_000,
        60_000,
    );
    for (factor_milli, extra_wait_rounds, terminal_from, steps) in [
        (2_000, 1, 6, vec![(0, Some(2_000), Some(1))]),
        (1_000, 0, 1, Vec::new()),
    ] {
        let root = TempDir::new().expect("tempdir");
        let path = root.path().join("one-tap.zip");
        fs::write(&path, &one_tap).expect("write package");
        let state = Arc::new(FakeState::default());
        // The device shows `home` before the tap, and after it `home` until the
        // `terminal_from`-th frame, `terminal` from then on.
        let first_after_tap = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
        *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
            move |capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
                let page = if inputs == 0 {
                    HOME
                } else {
                    let first = first_after_tap
                        .fetch_min(capture, Ordering::SeqCst)
                        .min(capture);
                    // This frame is the `nth` after the tap.
                    let nth = capture - first + 1;
                    if nth >= terminal_from { TERMINAL } else { HOME }
                };
                vec![(0, page)]
            },
        ));
        let clock = Arc::new(WaitClock::new(1_000));
        let host = direct_host(&root, &clock, &state);
        host.pin_step_pacing_for_test(ALIAS, factor_milli, extra_wait_rounds)
            .expect("pin the step pacing");

        let (receipt, events) = run_direct(&host, &path, 60_000);
        host.close().expect("close runtime host");

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "factor {factor_milli}: {receipt:?}"
        );
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        assert_eq!(step_entries(&events), steps, "factor {factor_milli}");
    }
}

// #385 P-k2, PK2 part 2 (host gate 111; register §1.108 PK2-4; PERF v1.4.2 §5.3, §5.6, review
// M3; the brief's requirements 1 and 2): every paced wait ends by the run's host deadline. A
// run's lease lasts the heartbeat reserve past its host deadline (`contained_task_lease_ttl`,
// `contained_task_deadline_until`) and is not renewed while the run waits, so a wait stretched by
// the factor that starts before the deadline must still end before the lease does: the run then
// ends `contained_task_deadline_exceeded` at its next check, inside the reserve, its lease is
// released, none lapses (the lease sweep never finds it due, so the terminal never meets
// `lease_mismatch`) and the host closes. Each run below is on the shipped scheduler, whose reserve
// is 5 000 ms, at a pinned factor of 4 000.

/// The pinned factor of tests 7 to 9.
const PACED_FACTOR_MILLI: u32 = 4_000;

/// The shipped heartbeat reserve: how long a run's lease lasts past its host deadline.
fn shipped_reserve_ms() -> u64 {
    let reserve_ms = SchedulerConfig::default().maximum_client_heartbeat_interval_ms;
    assert_eq!(reserve_ms, 5_000, "the shipped reserve");
    reserve_ms
}

/// One tap from `home` to `terminal`, which never arrives on a device that stays on `home`: the
/// confirmation captures a frame every 5 s (a wait of 20 s at 4 000) within a 50 s step and task
/// timeout, so the run ends at its host deadline.
fn never_arrives_package() -> Vec<u8> {
    package(
        &[("open_terminal", "home", "terminal", None)],
        5_000,
        50_000,
        50_000,
    )
}

/// The run ended at `ended_ms` on the Runtime clock, at or after its host deadline `deadline_ms`
/// and before its lease ends, `reserve_ms` later; its last wait, a paced wait of `stretched_ms`,
/// started before the deadline and would have ended at or after the lease's end had it run whole.
fn assert_ended_by_the_deadline(
    clock: &WaitClock,
    ended_ms: u64,
    deadline_ms: u64,
    reserve_ms: u64,
    stretched_ms: u64,
) {
    let waits = clock.waits_ms();
    assert!(
        (deadline_ms..deadline_ms + reserve_ms).contains(&ended_ms),
        "the run ended at {ended_ms} ms, its deadline {deadline_ms} ms: waits {waits:?}"
    );
    let last_wait_ms = *waits.last().expect("the run waited");
    let wait_started_ms = ended_ms - last_wait_ms;
    assert!(
        wait_started_ms < deadline_ms,
        "the last wait started at {wait_started_ms} ms: waits {waits:?}"
    );
    assert!(
        wait_started_ms + stretched_ms >= deadline_ms + reserve_ms,
        "whole, the wait from {wait_started_ms} ms would end before the lease: waits {waits:?}"
    );
}

/// The lease the run's terminal names (among `run_records`) is released once, no lease lapses and
/// no record names `lease_mismatch` (among `all_records`).
fn assert_lease_released(run_records: &[PersistedEvent], all_records: &[PersistedEvent]) {
    let lease = run_records
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::TaskCancelled | EventType::TaskFailed
            )
        })
        .find_map(|event| event.links().lease_id().copied())
        .expect("the run's terminal names its lease");
    let released = all_records
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(&lease)
        })
        .count();
    assert_eq!(released, 1, "the run's lease is released once");
    assert!(
        all_records
            .iter()
            .all(|event| event.event_type() != EventType::LeaseExpired),
        "no lease lapses"
    );
    assert!(
        !all_records.iter().any(|event| {
            serde_json::to_string(event.payload())
                .expect("payload JSON")
                .contains("lease_mismatch")
        }),
        "no lease_mismatch record"
    );
}

/// A direct run's receipt: cancelled, reason `deadline_exceeded`.
fn assert_cancelled_at_the_deadline(receipt: &RuntimeReceipt) {
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
}

// Test 7 (requirement 1; PK2-4): a direct run with `response_deadline_ms` 50 000 whose target page
// never arrives. After its 900 ms pre-input wait (the shipped base 300 ms × 3 000 / 1 000) and its
// tap, the confirmation captures a frame every 20 s: at 900, 20 900 and 40 900 ms. The wait from
// 40 900 ms would end at 60 900 ms, after the lease (50 000 + 5 000); it ends at the deadline
// instead, and the run is cancelled `deadline_exceeded` with the Runtime clock in
// [50 000, 55 000), its lease released, none lapsed, no `lease_mismatch`, and the host closes.
#[test]
fn pk2_a_paced_wait_ends_by_the_host_deadline() {
    const RESPONSE_DEADLINE_MS: u64 = 50_000;
    // A frame interval of 5 s at 4 000.
    const STRETCHED_WAIT_MS: u64 = 20_000;
    let reserve_ms = shipped_reserve_ms();
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("never-arrives.zip");
    fs::write(&path, never_arrives_package()).expect("write package");
    // The device stays on `home` after the tap.
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(WaitClock::new(1_000));
    let host = direct_host_with(
        config(&root).with_scheduler(SchedulerConfig::default()),
        &clock,
        &state,
    );
    host.pin_pacing_factor_for_test(ALIAS, PACED_FACTOR_MILLI)
        .expect("pin the factor");

    let (receipt, events) = run_direct(&host, &path, RESPONSE_DEADLINE_MS);
    let ended_ms = clock.monotonic_ms();
    let all_records = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("every record");
    host.close().expect("close runtime host");

    assert_cancelled_at_the_deadline(&receipt);
    assert_eq!(admitted_deadline(&events), Some(RESPONSE_DEADLINE_MS));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_ended_by_the_deadline(
        &clock,
        ended_ms,
        RESPONSE_DEADLINE_MS,
        reserve_ms,
        STRETCHED_WAIT_MS,
    );
    assert_lease_released(&events, &all_records);
}

/// A configuration that sets `performance.pacing.pre_input_wait_base_ms` and nothing else.
fn pre_input_wait_base(base_ms: u64) -> actingcommand_contract::RuntimeConfigManifest {
    let mut parameters = vec![actingcommand_contract::ConfigParameter {
        key: "performance.pacing.pre_input_wait_base_ms".to_owned(),
        value: actingcommand_contract::FactScalar::DurationMs(base_ms),
        source: actingcommand_contract::ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    actingcommand_contract::RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

// Test 8 (requirement 1, PK2's pre-input wait; PK2-4): a direct run with `response_deadline_ms`
// 2 000 and `pre_input_wait_base_ms` 3 000 waits 3 000 × 3 000 / 1 000 = 9 000 ms before its tap,
// from the start: whole, that wait would end after the lease (2 000 + 5 000). It ends at the
// deadline instead, and the run is cancelled `deadline_exceeded` before its tap with the Runtime
// clock in [2 000, 7 000), its lease released, none lapsed, no `lease_mismatch`, and the host
// closes.
#[test]
fn pk2_the_pre_input_wait_ends_by_the_host_deadline() {
    const RESPONSE_DEADLINE_MS: u64 = 2_000;
    const PRE_INPUT_WAIT_BASE_MS: u64 = 3_000;
    const PRE_INPUT_WAIT_MS: u64 = 9_000;
    let reserve_ms = shipped_reserve_ms();
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("one-tap.zip");
    fs::write(
        &path,
        package(
            &[("open_terminal", "home", "terminal", None)],
            1,
            5_000,
            60_000,
        ),
    )
    .expect("write package");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(WaitClock::new(1_000));
    let host = direct_host_with(
        config(&root)
            .with_scheduler(SchedulerConfig::default())
            .with_config_manifest(pre_input_wait_base(PRE_INPUT_WAIT_BASE_MS)),
        &clock,
        &state,
    );
    host.pin_pacing_factor_for_test(ALIAS, PACED_FACTOR_MILLI)
        .expect("pin the factor");

    let (receipt, events) = run_direct(&host, &path, RESPONSE_DEADLINE_MS);
    let ended_ms = clock.monotonic_ms();
    let all_records = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("every record");
    host.close().expect("close runtime host");

    assert_cancelled_at_the_deadline(&receipt);
    assert_eq!(admitted_deadline(&events), Some(RESPONSE_DEADLINE_MS));
    assert_eq!(
        state.input_count.load(Ordering::Acquire),
        0,
        "the run ends before its tap"
    );
    assert_ended_by_the_deadline(
        &clock,
        ended_ms,
        RESPONSE_DEADLINE_MS,
        reserve_ms,
        PRE_INPUT_WAIT_MS,
    );
    assert_lease_released(&events, &all_records);
}

// Test 9 (requirement 2; PK2-4): a scheduled run's lease is taken at its admission for
// `response_deadline_ms` + the reserve and is not renewed while the run runs; its deadline is at
// most the lease's end less the reserve. So it could lapse as a direct run could, and its waits
// end by its deadline the same way. Test 7's run as a scheduled run, admitted at 0 ms: its lease
// ends at 55 000 ms, its deadline is 50 000 ms, and the confirmation's wait from 40 900 ms would
// end at 60 900 ms. It ends `contained_task_deadline_exceeded` through the failure settlement (one
// failed terminal, one release, one execution record) with the Runtime clock in [50 000, 55 000),
// no lease lapsed, no `lease_mismatch`, and the host closes.
#[test]
fn pk2_a_scheduled_runs_paced_wait_ends_by_its_deadline() {
    const RESPONSE_DEADLINE_MS: u64 = 50_000;
    const STRETCHED_WAIT_MS: u64 = 20_000;
    let reserve_ms = shipped_reserve_ms();
    let package = never_arrives_package();
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("scheduled-never-arrives.zip");
    fs::write(&path, &package).expect("write package");
    // The device stays on `home` after the tap.
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(WaitClock::new(POLICY_NOW_UNIX_MS));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig::default())
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
    clock.sample_capacity_after_moves(host.capacity_sampler_for_test().expect("capacity owner"));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the policy catalog");
    host.pin_pacing_factor_for_test(POLICY_INSTANCE_ALIAS, PACED_FACTOR_MILLI)
        .expect("pin the factor");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let request = ContainedTaskRequest::new(path.to_string_lossy().into_owned())
        .expect("contained task request")
        .with_response_deadline_ms(RESPONSE_DEADLINE_MS)
        .expect("bounded response deadline");
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
    assert_eq!(
        context.lease_token().expires_at_monotonic_ms(),
        admitted_ms + RESPONSE_DEADLINE_MS + reserve_ms,
        "the lease lasts the reserve past the deadline"
    );

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the run reaches its deadline");
    let ended_ms = clock.monotonic_ms();
    let run_records = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        })
        .expect("the run's records");
    let all_records = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("every record");
    host.close().expect("close runtime host");

    assert_eq!(
        error.code(),
        "contained_task_deadline_exceeded",
        "{error:?}"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    for event_type in [
        EventType::TaskFailed,
        EventType::LeaseReleased,
        EventType::PolicyExecutionRecorded,
    ] {
        assert_eq!(
            run_records
                .iter()
                .filter(|event| event.event_type() == event_type)
                .count(),
            1,
            "one {event_type:?}"
        );
    }
    assert_ended_by_the_deadline(
        &clock,
        ended_ms,
        admitted_ms + RESPONSE_DEADLINE_MS,
        reserve_ms,
        STRETCHED_WAIT_MS,
    );
    assert_lease_released(&run_records, &all_records);
}
