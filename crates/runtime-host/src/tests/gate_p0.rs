// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #385 P-0 (performance model v1.4.1 §14, T0-1 to T0-5): the levels below Suspended
//! never stop dispatch, urgency never turns a deferral into a failure, the host budget is not
//! scaled by the level, and `capture_latency_ms` alone scores no responsiveness. Every time is an
//! explicit unix ms; the hosts run on a `ManualRuntimeClock`.

use super::*;
use crate::performance::PerformanceMonitor;
use crate::performance_control::{PerformanceBalanceController, PerformanceDispatchGate};
use actingcommand_contract::PerformanceDeadlineDisposition;
use actingcommand_host_metrics::{HostSample, HostSampler, ProcessLoadThresholds};

const INSTANCE: &str = "fixture-instance";
/// The spacing of host samples: the default hysteresis takes two samples per level and a 4 s
/// cooldown, so one level per two samples.
const HOST_SAMPLE_SPACING_MS: u64 = 2_000;

fn controller() -> PerformanceBalanceController {
    PerformanceBalanceController::new(PerformanceControlConfig::default().with_hysteresis(
        1,
        1,
        Duration::from_millis(1),
    ))
    .expect("performance controller")
}

fn responsiveness(at: u64, basis_points: u16) -> PerformanceControlObservation {
    PerformanceControlObservation {
        observed_at_unix_ms: at,
        host_responsiveness_basis_points: Some(basis_points),
        third_party_pressure_basis_points: None,
        foreground_fullscreen: false,
    }
}

/// A host on a manual clock whose fixture goal is due now (urgency 1000), as the retired
/// admission test set it up.
fn deadline_host(root: &TempDir) -> RuntimeHost {
    let host = manual_clock_host(root);
    let mut sources = policy_sources(1);
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("activity fixture");
    activity["profiles"][0]["goals"][0]["deadline_unix_ms"] = serde_json::json!(POLICY_NOW_UNIX_MS);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate deadline catalog");
    host
}

fn manual_clock_host(root: &TempDir) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

/// Raises the host's controller by `levels` levels with host responsiveness samples under the
/// default hysteresis; the last sample is at `POLICY_NOW_UNIX_MS`.
fn raise_host(host: &RuntimeHost, basis_points: u16, levels: u64) {
    let samples = levels * 2;
    let first_at = POLICY_NOW_UNIX_MS - (samples - 1) * HOST_SAMPLE_SPACING_MS;
    for index in 0..samples {
        host.observe_performance_control_for_test(responsiveness(
            first_at + index * HOST_SAMPLE_SPACING_MS,
            basis_points,
        ))
        .expect("host responsiveness sample");
    }
}

fn host_level(host: &RuntimeHost) -> PerformanceControlLevel {
    host.performance_control_directive(POLICY_INSTANCE_ALIAS)
        .expect("directive")
        .level
}

/// T0-1 (controller): DispatchPaused, Throttled, YieldRequested and QosReduced allow every
/// dispatch at urgency 0 and 1000, also on an immediate second call (no window, no wait).
#[test]
fn gate_p0_t01_levels_below_suspended_allow_dispatch() {
    let mut controller = controller();
    let mut at = 1_000;
    for (basis_points, level) in [
        (8_000, PerformanceControlLevel::DispatchPaused),
        (7_000, PerformanceControlLevel::Throttled),
        (6_000, PerformanceControlLevel::YieldRequested),
        (5_000, PerformanceControlLevel::QosReduced),
    ] {
        at += 10;
        controller
            .observe(responsiveness(at, basis_points), &[])
            .expect("pressure sample");
        assert_eq!(
            controller.directive(INSTANCE).expect("directive").level,
            level
        );
        for urgency_milli in [0, 1_000] {
            for call in ["first", "immediate second"] {
                assert_eq!(
                    controller
                        .gate_dispatch(INSTANCE, urgency_milli, at)
                        .expect("gate"),
                    PerformanceDispatchGate::Allowed,
                    "{level:?}, urgency {urgency_milli}, {call} call"
                );
            }
        }
    }
}

/// T0-1 (host): at DispatchPaused an urgent dispatch is admitted and nothing records a
/// rejection or a deadline conflict.
#[test]
fn gate_p0_t01_dispatch_paused_admission_records_no_rejection() {
    let root = TempDir::new().expect("tempdir");
    let host = deadline_host(&root);
    raise_host(&host, 8_000, 1);
    assert_eq!(host_level(&host), PerformanceControlLevel::DispatchPaused);

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.prerequisites.urgency_milli, 1_000);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("DispatchPaused admits the urgent dispatch");

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchRejected),
        "no rejection record"
    );
    assert!(
        !events.iter().any(|event| {
            event.event_type == EventType::PerformanceBalanceChanged
                && serde_json::to_string(event)
                    .expect("event JSON")
                    .contains("\"deadline_conflict\"")
        }),
        "no DeadlineConflict balance event"
    );
    drop(client);
    host.close().expect("close host");
}

/// T0-2: at Suspended an urgency-1000 dispatch is deferred, never failed as a deadline
/// conflict, in the controller and through admission.
#[test]
fn gate_p0_t02_suspended_urgent_dispatch_defers_without_deadline_failure() {
    let mut controller = controller();
    for step in 0..5 {
        controller
            .observe(responsiveness(1_000 + step * 10, 4_000), &[])
            .expect("pressure sample");
    }
    assert_eq!(
        controller.directive(INSTANCE).expect("directive").level,
        PerformanceControlLevel::Suspended
    );
    let PerformanceDispatchGate::Deferred {
        reason,
        deadline_disposition,
        ..
    } = controller
        .gate_dispatch(INSTANCE, 1_000, 1_100)
        .expect("gate")
    else {
        panic!("Suspended defers the dispatch");
    };
    assert_eq!(reason.as_str(), "performance_contention_dispatch_deferred");
    assert_ne!(
        deadline_disposition,
        Some(PerformanceDeadlineDisposition::CapacityFailure)
    );

    let root = TempDir::new().expect("tempdir");
    let host = deadline_host(&root);
    raise_host(&host, 4_000, 5);
    assert_eq!(host_level(&host), PerformanceControlLevel::Suspended);
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.prerequisites.urgency_milli, 1_000);
    record_policy_approval(&host, &intent);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("Suspended defers the dispatch");
    assert!(!error.is_fatal());
    assert_eq!(error.code(), "performance_contention_dispatch_deferred");

    let mut client = TestClient::connect(&host);
    for event in projected_events(&mut client, EventQuery::default()) {
        let json = serde_json::to_string(&event).expect("event JSON");
        assert!(
            !json.contains("performance_capacity_deadline_conflict"),
            "{:?} carries the deadline failure code",
            event.event_type
        );
        assert!(
            !json.contains("\"capacity_failure\""),
            "{:?} carries a CapacityFailure disposition",
            event.event_type
        );
    }
    drop(client);
    host.close().expect("close host");
}

/// T0-3: at QosReduced a host with 199 left keeps 199, so a light candidate (100 each) is
/// dispatched, not deferred for the host budget.
#[test]
fn gate_p0_t03_qos_reduced_keeps_host_budget_for_light() {
    let mut controller = controller();
    for step in 0..4 {
        controller
            .observe(responsiveness(1_000 + step * 10, 5_000), &[])
            .expect("pressure sample");
    }
    assert_eq!(
        controller.directive(INSTANCE).expect("directive").level,
        PerformanceControlLevel::QosReduced
    );
    let mut constrained = policy_resources();
    for host in &mut constrained.hosts {
        host.cpu_available_milli = 199;
        host.gpu_available_milli = 199;
        host.io_available_milli = 199;
    }
    let mut overlaid = constrained.hosts.clone();
    controller
        .apply_to_resources(&mut overlaid, &[])
        .expect("overlay");
    for host in &overlaid {
        assert_eq!(
            (
                host.cpu_available_milli,
                host.gpu_available_milli,
                host.io_available_milli
            ),
            (199, 199, 199)
        );
    }

    let root = TempDir::new().expect("tempdir");
    let host = manual_clock_host(&root);
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("task fixture");
    tasks["tasks"][0]["load_profile"] = serde_json::json!({"kind": "light"});
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate light catalog");
    raise_host(&host, 5_000, 4);
    assert_eq!(host_level(&host), PerformanceControlLevel::QosReduced);
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &constrained,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("policy evaluation");
    let evaluation = cycle.evaluation.as_ref().expect("evaluation");
    assert!(
        !evaluation
            .decisions
            .iter()
            .flat_map(|decision| &decision.reasons)
            .any(|reason| reason.code == "host_budget_deferred"),
        "no candidate is deferred for the host budget: {evaluation:#?}"
    );
    assert!(
        evaluation.dispatch_intents.iter().any(|intent| {
            intent.task_id == "fixture.observe" && intent.instance_id == POLICY_INSTANCE_ALIAS
        }),
        "the light candidate yields a dispatch intent: {evaluation:#?}"
    );
    host.close().expect("close host");
}

/// A sampler the T0-4 and T0-5 monitors never call: they take no tick.
struct NoHostSample;

impl HostSampler for NoHostSample {
    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
        unreachable!("the gate takes no host sample")
    }
}

fn pipeline_monitor() -> PerformanceMonitor {
    PerformanceMonitor::enabled(
        PerformanceMonitorConfig::default(),
        Ok(Box::new(NoHostSample)),
    )
    .expect("performance monitor")
}

/// T0-4 (performance research §4.3: `capture_latency_ms=1336` scored 3742): a signal with only
/// a capture latency carries no host responsiveness.
#[test]
fn gate_p0_t04_capture_latency_alone_scores_nothing() {
    let mut monitor = pipeline_monitor();
    monitor
        .record_pipeline_signal(
            PipelinePerformanceSignal::measured(INSTANCE, POLICY_NOW_UNIX_MS)
                .with_capture_latency(1_336)
                .expect("capture latency"),
        )
        .expect("pipeline sample");
    let observation = monitor
        .control_observation(POLICY_NOW_UNIX_MS)
        .expect("control observation");
    assert_eq!(
        observation.and_then(|observation| observation.host_responsiveness_basis_points),
        None
    );
}

/// T0-5 (performance-timing-signals.md): a 7.198 ms acquisition and a 51 ms touch response
/// both score full responsiveness.
#[test]
fn gate_p0_t05_fast_acquire_and_touch_score_full() {
    let mut monitor = pipeline_monitor();
    monitor
        .record_pipeline_signal(
            PipelinePerformanceSignal::measured(INSTANCE, POLICY_NOW_UNIX_MS)
                .with_capture_acquire(7_198)
                .expect("capture acquire")
                .with_touch_response(51_000)
                .expect("touch response"),
        )
        .expect("pipeline sample");
    let observation = monitor
        .control_observation(POLICY_NOW_UNIX_MS)
        .expect("control observation");
    assert_eq!(
        observation.and_then(|observation| observation.host_responsiveness_basis_points),
        Some(10_000)
    );
}
