// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #385 PM-CFG (wave-17 ruling 10; frozen model PERF v1.4.2 §10.1; register §5 "Read by
// P-m"): the monitor, its sampler and the controller act on every value of the monitor's row in
// effect. Each case runs one input under the row's defaults (§10.1, the constants the monitor
// ran with before) and under a configuration that names another value of one key, and expects
// what that value says: a sampling period, a context window, a summary period, a bound on the
// samples kept, the failures in a row that stop sampling, the controller's freshness window as
// that many sampling periods, the frame gap of a stutter, where each pressure starts and ends,
// how severe it is, the process thresholds and coverage the sampler is asked for, and the
// summed load of a heavy workload, which the controller and the policy evaluator judge by the
// same value (one case runs through the host's policy cycle). The settings are read from a
// configuration file's `performance` section as a start reads them. Times are fixed Unix
// milliseconds; nothing here sleeps or reads a clock (the host's Runtime clock is a manual one).

use super::{
    FakeProvider, FakeState, ManualRuntimeClock, POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B,
    POLICY_NOW_UNIX_MS, TestClient, config, instance_id, pending_policy_facts,
    pending_policy_sources, policy_context, policy_resources, record_policy_approval,
};
use crate::performance::{
    PerformanceMonitor, PerformanceMonitorConfig, PerformanceSemanticEvent,
    PipelinePerformanceSignal,
};
use crate::performance_control::{
    PerformanceBalanceController, PerformanceControlConfig, PerformanceControlWorkload,
};
use crate::{PolicyDispatchAdmission, PolicyTrigger, RuntimeHost};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    PerformancePressureKind, PerformancePressureSeverity, RuntimeOperation, RuntimeReceiptState,
};
use actingcommand_host_metrics::{
    GpuAggregate, HostMetric, HostMetricsCode, HostSample, HostSampler, ProcessLoadThresholds,
    ProcessMetricCoverage, ProcessOwnership, ProcessSample,
};
use actingcommand_policy::{
    EvaluationTime, HostResourceSnapshot, LoadProfile, SchedulingDecisionState,
};
use actingcommand_runtime_config::performance::monitor::MonitorSettings;
use actingcommand_runtime_config::{DefaultsFile, mechanism_rows, render_parameters};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";

// The monitor's row as a start reads it from a file whose `performance` section is `section`.
fn settings(section: Value) -> MonitorSettings {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file");
    let parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "performance": section }),
        &|_| None,
    )
    .expect("every key resolves");
    MonitorSettings::parse(&|key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .map(|parameter| parameter.value.clone())
    })
    .expect("the monitor's row")
}

// The monitor's configuration from that row.
fn monitor_config(section: Value) -> PerformanceMonitorConfig {
    PerformanceMonitorConfig::default().with_settings(&settings(section))
}

// Hands out its samples in order and keeps the process thresholds each call asked for.
struct ScriptedSampler {
    samples: VecDeque<Result<HostSample, Code>>,
    asked: Arc<Mutex<Vec<ProcessLoadThresholds>>>,
}

impl HostSampler for ScriptedSampler {
    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, Code> {
        self.asked
            .lock()
            .expect("the thresholds asked for")
            .push(thresholds);
        self.samples
            .pop_front()
            .expect("the scripted samples are enough")
    }
}

fn monitor_with(
    config: PerformanceMonitorConfig,
    samples: Vec<Result<HostSample, Code>>,
) -> (PerformanceMonitor, Arc<Mutex<Vec<ProcessLoadThresholds>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let monitor = PerformanceMonitor::enabled(
        config,
        Ok(Box::new(ScriptedSampler {
            samples: samples.into(),
            asked: Arc::clone(&asked),
        })),
    )
    .expect("the monitor starts");
    (monitor, asked)
}

fn monitor_for(section: Value, samples: Vec<Result<HostSample, Code>>) -> PerformanceMonitor {
    monitor_with(monitor_config(section), samples).0
}

// One host sample: the CPU, RAM, GPU and disk readings given, every process readable.
#[derive(Clone, Copy)]
struct Reading {
    cpu: u16,
    ram: u16,
    gpu: Option<u16>,
    disk_queue_milli: u32,
    disk_latency_us: u64,
}

const CALM: Reading = Reading {
    cpu: 1_000,
    ram: 4_000,
    gpu: None,
    disk_queue_milli: 500,
    disk_latency_us: 1_000,
};

fn host_sample(at: u64, reading: Reading, third_party: Vec<ProcessSample>) -> HostSample {
    let mut unavailable_metrics = vec![HostMetric::ForegroundProcess];
    if reading.gpu.is_none() {
        unavailable_metrics.push(HostMetric::Gpu);
    }
    HostSample {
        observed_at_unix_ms: at,
        cpu_total_basis_points: reading.cpu,
        cpu_per_core_basis_points: vec![reading.cpu],
        ram_used_basis_points: reading.ram,
        disk_queue_depth_milli: Some(reading.disk_queue_milli),
        disk_latency_micros: Some(reading.disk_latency_us),
        gpu_basis_points: reading.gpu,
        foreground: None,
        owned_processes: Vec::new(),
        third_party_high_load: third_party,
        process_coverage: ProcessMetricCoverage {
            enumerated_processes: 1,
            sampled_processes: 1,
            audited_protected_processes_excluded: 0,
            unconfirmed_access_denied_processes: 0,
            unexpectedly_inaccessible_processes: 0,
            readable_basis_points: 10_000,
        },
        unavailable_metrics,
    }
}

// Another program's process under high load.
fn other_process(pid: u32, cpu: u16) -> ProcessSample {
    ProcessSample {
        pid,
        process_name: format!("neutral-process-{pid}"),
        ownership: ProcessOwnership::ThirdParty,
        cpu_basis_points: cpu,
        working_set_bytes: 1_024,
        peak_working_set_bytes: None,
        process_created_at_windows_100ns: None,
        io_bytes_per_second: 0,
    }
}

// `count` copies of `item`.
fn repeated<T: Clone>(item: T, count: usize) -> Vec<T> {
    vec![item; count]
}

// The samples of `readings`, one every two seconds from 1 000.
fn samples(readings: &[(Reading, Vec<ProcessSample>)]) -> Vec<Result<HostSample, Code>> {
    readings
        .iter()
        .zip(0_u64..)
        .map(|((reading, third_party), index)| {
            Ok(host_sample(
                1_000 + index * 2_000,
                *reading,
                third_party.clone(),
            ))
        })
        .collect()
}

// Every event of one tick per sample.
fn run(monitor: &mut PerformanceMonitor, count: usize) -> Vec<PerformanceSemanticEvent> {
    (0_u64..)
        .take(count)
        .flat_map(|index| {
            monitor
                .tick(1_000 + index * 2_000)
                .expect("the sample is ingested")
                .events
        })
        .collect()
}

fn started(
    events: &[PerformanceSemanticEvent],
) -> BTreeMap<PerformancePressureKind, PerformancePressureSeverity> {
    events
        .iter()
        .filter_map(|event| match event {
            PerformanceSemanticEvent::PressureStarted(data) => {
                Some((data.pressure.kind, data.pressure.severity))
            }
            _ => None,
        })
        .collect()
}

fn ended(events: &[PerformanceSemanticEvent]) -> BTreeSet<PerformancePressureKind> {
    events
        .iter()
        .filter_map(|event| match event {
            PerformanceSemanticEvent::PressureEnded(data) => Some(data.pressure.kind),
            _ => None,
        })
        .collect()
}

#[test]
fn pmcfg_the_monitor_takes_its_period_windows_bounds_and_failure_limit_from_configuration() {
    // `monitor.sample_interval_ms`: the period the host samples at.
    for (section, expected) in [
        (json!({}), Duration::from_millis(2_000)),
        (
            json!({"monitor": {"sample_interval_ms": 3_000}}),
            Duration::from_millis(3_000),
        ),
    ] {
        assert_eq!(
            monitor_for(section, Vec::new()).sample_interval(),
            Some(expected)
        );
    }

    // `monitor.context_window_ms`: samples at 1 000 and 12 000; a context ending at 12 000 holds
    // both within 30 s, only the second within 10 s.
    for (section, expected) in [
        (json!({}), 2),
        (json!({"monitor": {"context_window_ms": 10_000}}), 1),
    ] {
        let mut monitor = monitor_for(
            section,
            vec![
                Ok(host_sample(1_000, CALM, Vec::new())),
                Ok(host_sample(12_000, CALM, Vec::new())),
            ],
        );
        monitor.tick(1_000).expect("first sample");
        monitor.tick(12_000).expect("second sample");
        assert_eq!(
            monitor
                .context(ALIAS, 12_000)
                .expect("the context")
                .sample_count,
            expected
        );
    }

    // `monitor.max_system_samples`: three samples within the window; 64 keeps them all, 2 keeps
    // the last two.
    for (section, expected) in [
        (json!({}), 3),
        (json!({"monitor": {"max_system_samples": 2}}), 2),
    ] {
        let mut monitor = monitor_for(section, samples(&repeated((CALM, Vec::new()), 3)));
        run(&mut monitor, 3);
        assert_eq!(
            monitor
                .context(ALIAS, 5_000)
                .expect("the context")
                .sample_count,
            expected
        );
    }

    // `monitor.max_pipeline_samples`: frame gaps of 900, 200 and 300 ms; 4 096 keeps the 900 ms
    // one, 2 keeps only the last two.
    for (section, expected) in [
        (json!({}), 900),
        (json!({"monitor": {"max_pipeline_samples": 2}}), 300),
    ] {
        let mut monitor = monitor_for(section, Vec::new());
        for (at, gap) in [(1_000, 900), (1_100, 200), (1_200, 300)] {
            monitor
                .record_pipeline_signal(
                    PipelinePerformanceSignal::new(ALIAS, at, gap).expect("a frame gap"),
                )
                .expect("the pipeline sample");
        }
        assert_eq!(
            monitor
                .context(ALIAS, 1_200)
                .expect("the context")
                .max_frame_gap_ms,
            Some(expected)
        );
    }

    // `monitor.summary_interval_ms`: samples at 1 000, 3 000 and 5 000; the first is always
    // summarised, the third only when the period is 4 s rather than 60 s.
    for (section, expected) in [
        (json!({}), 1),
        (json!({"monitor": {"summary_interval_ms": 4_000}}), 2),
    ] {
        let mut monitor = monitor_for(section, samples(&repeated((CALM, Vec::new()), 3)));
        let summaries = run(&mut monitor, 3)
            .iter()
            .filter(|event| matches!(event, PerformanceSemanticEvent::Summary(_)))
            .count();
        assert_eq!(summaries, expected);
    }

    // `monitor.max_consecutive_failures`: the sampler fails twice; the second failure stops
    // sampling when two are allowed, not when three are.
    for (section, terminal) in [
        (json!({}), false),
        (json!({"monitor": {"max_consecutive_failures": 2}}), true),
    ] {
        let failure = || Err(HostMetricsCode::PerformanceSystemTimesFailed.code());
        let mut monitor = monitor_for(section, vec![failure(), failure()]);
        assert!(!monitor.tick(1_000).expect("first failure").stop_sampling);
        let second = monitor.tick(3_000).expect("second failure");
        assert_eq!(second.stop_sampling, terminal);
        assert_eq!(
            second.events.iter().any(|event| matches!(
                event,
                PerformanceSemanticEvent::MonitorDegraded(data) if data.terminal
            )),
            terminal
        );
    }

    // `pressure.gpu_aggregate`: the GPU's engines as the sampler is opened with.
    for (section, expected) in [
        (json!({}), GpuAggregate::MaxEngineType),
        (
            json!({"pressure": {"gpu_aggregate": "sum"}}),
            GpuAggregate::Sum,
        ),
    ] {
        assert_eq!(monitor_config(section).gpu_aggregate(), expected);
    }
}

#[test]
fn pmcfg_the_controller_s_freshness_window_is_the_configured_number_of_sampling_periods() {
    // A slow recognition at 1 000 and no host sample; the controller reads at 6 000. Two
    // periods of 2 s leave it 1 s too old; three leave it fresh.
    for (section, fresh) in [
        (json!({}), false),
        (
            json!({"monitor": {"control_freshness_multiplier": 3}}),
            true,
        ),
    ] {
        let mut monitor = monitor_for(section, Vec::new());
        monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::measured(ALIAS, 1_000)
                    .with_recognition_latency(2_000)
                    .expect("a measured recognition"),
            )
            .expect("the pipeline sample");
        let observation = monitor
            .control_observation(6_000)
            .expect("the controller's read");
        assert_eq!(observation.is_some(), fresh);
    }
}

#[test]
fn pmcfg_a_stutter_is_a_frame_gap_from_the_configured_one() {
    // A 600 ms frame gap is no stutter from 1 000 ms, a stutter from 500 ms.
    for (section, stutter) in [
        (json!({}), false),
        (json!({"monitor": {"stutter_frame_gap_ms": 500}}), true),
    ] {
        let mut monitor = monitor_for(section, Vec::new());
        let events = monitor
            .record_pipeline_signal(
                PipelinePerformanceSignal::new(ALIAS, 1_000, 600).expect("a frame gap"),
            )
            .expect("the pipeline sample");
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, PerformanceSemanticEvent::StutterDetected(_))),
            stutter
        );
    }
}

#[test]
fn pmcfg_each_pressure_starts_and_ends_at_its_configured_threshold() {
    use PerformancePressureKind::{Cpu, DiskIo, Gpu, Ram};
    // Every kind starts lower and ends lower than its default (cpu [8 500, 7 000], ram
    // [9 000, 8 000], gpu [9 000, 7 500], disk queue [4 000, 2 000] milli, disk latency
    // [50 000, 25 000] us).
    let named = json!({"pressure": {"thresholds": {
        "cpu": [7_500, 6_000], "ram": [8_000, 7_000], "gpu": [8_000, 6_500],
        "disk_queue_milli": [2_500, 1_500], "disk_latency_us": [30_000, 20_000],
        "process_cpu_bp": 3_000, "process_io_bps": 33_554_432
    }}});
    let reading = |cpu, ram, gpu, disk_queue_milli, disk_latency_us| Reading {
        cpu,
        ram,
        gpu: Some(gpu),
        disk_queue_milli,
        disk_latency_us,
    };
    // Between each configured start and its default.
    let between_starts = reading(8_000, 8_500, 8_500, 3_000, 1_000);
    // Above both starts.
    let above = reading(9_000, 9_100, 9_100, 4_500, 1_000);
    // Between each configured end and its default.
    let between_ends = reading(6_500, 7_500, 7_000, 1_800, 1_000);
    // Disk latency alone between its configured start and its default.
    let latency_between_starts = reading(1_000, 4_000, 1_000, 500, 40_000);
    // Disk latency alone above both starts, then between both ends.
    let latency_above = reading(1_000, 4_000, 1_000, 500, 60_000);
    let latency_between_ends = reading(1_000, 4_000, 1_000, 500, 22_000);
    let all = BTreeSet::from([Cpu, Ram, Gpu, DiskIo]);
    for (case, readings, by_default, configured) in [
        (
            "between the starts",
            repeated(between_starts, 3),
            (BTreeSet::new(), BTreeSet::new()),
            (all.clone(), BTreeSet::new()),
        ),
        (
            "started, then between the ends",
            [repeated(above, 3), repeated(between_ends, 3)].concat(),
            (all.clone(), all.clone()),
            (all.clone(), BTreeSet::new()),
        ),
        (
            "a disk latency between the starts",
            repeated(latency_between_starts, 3),
            (BTreeSet::new(), BTreeSet::new()),
            (BTreeSet::from([DiskIo]), BTreeSet::new()),
        ),
        (
            "a disk latency started, then between the ends",
            [
                repeated(latency_above, 3),
                repeated(latency_between_ends, 3),
            ]
            .concat(),
            (BTreeSet::from([DiskIo]), BTreeSet::from([DiskIo])),
            (BTreeSet::from([DiskIo]), BTreeSet::new()),
        ),
    ] {
        let readings = readings
            .into_iter()
            .map(|reading| (reading, Vec::new()))
            .collect::<Vec<_>>();
        for (section, (expected_started, expected_ended)) in
            [(json!({}), by_default), (named.clone(), configured)]
        {
            let mut monitor = monitor_for(section.clone(), samples(&readings));
            let events = run(&mut monitor, readings.len());
            assert_eq!(
                started(&events).into_keys().collect::<BTreeSet<_>>(),
                expected_started,
                "{case}: {section}"
            );
            assert_eq!(ended(&events), expected_ended, "{case}: {section}");
        }
    }
}

#[test]
fn pmcfg_a_pressure_s_severity_follows_the_configured_levels() {
    use PerformancePressureSeverity::{Critical, Elevated, High};
    // Defaults: utilisation critical from 9 700, high from 9 200; other programs critical from
    // 3 processes or 8 000 bp, high from 2 processes or 6 000 bp.
    let named = json!({"pressure": {"severity": {
        "critical_bp": 9_400, "elevated_bp": 8_800,
        "third_party_critical": [4, 9_000], "third_party_elevated": [3, 7_000]
    }}});
    let busy = |cpu, ram| Reading { cpu, ram, ..CALM };
    let others = |peaks: &[u16]| {
        (1_u32..)
            .zip(peaks)
            .map(|(pid, cpu)| other_process(pid, *cpu))
            .collect::<Vec<_>>()
    };
    for (case, reading, third_party, kind, by_default, configured) in [
        (
            "a CPU at 9 500",
            busy(9_500, 4_000),
            Vec::new(),
            PerformancePressureKind::Cpu,
            High,
            Critical,
        ),
        (
            "a RAM at 9 100",
            busy(1_000, 9_100),
            Vec::new(),
            PerformancePressureKind::Ram,
            Elevated,
            High,
        ),
        (
            "three other processes at 4 500",
            CALM,
            others(&[4_500, 4_500, 4_500]),
            PerformancePressureKind::ThirdParty,
            Critical,
            High,
        ),
        (
            "one other process at 8 500",
            CALM,
            others(&[8_500]),
            PerformancePressureKind::ThirdParty,
            Critical,
            High,
        ),
        (
            "two other processes at 5 000",
            CALM,
            others(&[5_000, 5_000]),
            PerformancePressureKind::ThirdParty,
            High,
            Elevated,
        ),
        (
            "one other process at 6 500",
            CALM,
            others(&[6_500]),
            PerformancePressureKind::ThirdParty,
            High,
            Elevated,
        ),
    ] {
        let readings = repeated((reading, third_party), 3);
        for (section, expected) in [(json!({}), by_default), (named.clone(), configured)] {
            let mut monitor = monitor_for(section.clone(), samples(&readings));
            let events = run(&mut monitor, readings.len());
            assert_eq!(
                started(&events).get(&kind),
                Some(&expected),
                "{case}: {section}"
            );
        }
    }
}

#[test]
fn pmcfg_the_sampler_is_asked_for_the_configured_process_thresholds_and_coverage() {
    for (section, expected) in [
        (
            json!({}),
            ProcessLoadThresholds {
                cpu_basis_points: 4_000,
                io_bytes_per_second: 67_108_864,
                min_coverage_basis_points: 9_900,
            },
        ),
        (
            json!({
                "monitor": {"process_coverage_min_bp": 9_800},
                "pressure": {"thresholds": {
                    "cpu": [8_500, 7_000], "ram": [9_000, 8_000], "gpu": [9_000, 7_500],
                    "disk_queue_milli": [4_000, 2_000], "disk_latency_us": [50_000, 25_000],
                    "process_cpu_bp": 3_000, "process_io_bps": 33_554_432
                }}
            }),
            ProcessLoadThresholds {
                cpu_basis_points: 3_000,
                io_bytes_per_second: 33_554_432,
                min_coverage_basis_points: 9_800,
            },
        ),
    ] {
        let (mut monitor, asked) = monitor_with(
            monitor_config(section),
            vec![Ok(host_sample(1_000, CALM, Vec::new()))],
        );
        monitor.tick(1_000).expect("the sample");
        assert_eq!(
            *asked.lock().expect("the thresholds asked for"),
            vec![expected]
        );
    }
}

#[test]
fn pmcfg_the_controller_counts_heavy_work_from_the_configured_threshold() {
    // A workload of 400 + 400 + 400 milli: below the default 1 500, heavy from 1 000.
    for (section, expected) in [
        (json!({}), 0),
        (json!({"load": {"heavy_threshold_milli": 1_000}}), 1),
    ] {
        let config = PerformanceControlConfig::default()
            .with_heavy_threshold_milli(settings(section).heavy_threshold_milli);
        let controller = PerformanceBalanceController::new(config).expect("the controller");
        let mut resources = [HostResourceSnapshot {
            host_id: "neutral-host".to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 2,
            active_heavy_dispatches: 0,
            heavy_threshold_milli: actingcommand_policy::DEFAULT_HEAVY_THRESHOLD_MILLI,
        }];
        controller
            .apply_to_resources(
                &mut resources,
                &[PerformanceControlWorkload {
                    instance_id: ALIAS.to_owned(),
                    load_profile: LoadProfile::Weighted {
                        cpu_milli: 400,
                        gpu_milli: 400,
                        io_milli: 400,
                    },
                    arbitration: None,
                }],
            )
            .expect("the overlay");
        assert_eq!(resources[0].active_heavy_dispatches, expected);
    }
}

#[test]
fn pmcfg_the_controller_and_the_evaluator_judge_heavy_work_by_the_one_configured_threshold() {
    // End to end through the host. Two instances share one host whose heavy-dispatch budget is
    // one (both limits 1). Each of the catalog's two tasks weighs 200 + 100 + 300 = 600 milli:
    // light under the default 1 500, heavy from 600. A's dispatch is admitted while B is held,
    // then B is released and the next cycle judges B's candidate. Under the default neither
    // A's admitted work nor B's candidate is heavy, so B dispatches. At 600 the controller counts
    // A's work as the host's one heavy dispatch and the evaluator calls B's candidate heavy, so
    // B is blocked `heavy_scene_budget_exhausted`; if either judged by another threshold, B
    // would dispatch.
    for (section, b_dispatches) in [
        (json!({}), true),
        (json!({"load": {"heavy_threshold_milli": 600}}), false),
    ] {
        let root = TempDir::new().expect("tempdir");
        let control = PerformanceControlConfig::default()
            .with_heavy_dispatch_limits(1, 1)
            .with_heavy_threshold_milli(settings(section).heavy_threshold_milli);
        // The host's Runtime clock stands at the fixture's time (board #71).
        let host = RuntimeHost::start(
            config(&root)
                .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
                .with_performance_control(control),
            Arc::new(FakeProvider::from_entries([
                (
                    POLICY_INSTANCE_ALIAS.to_owned(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                ),
                (
                    POLICY_INSTANCE_ALIAS_B.to_owned(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                ),
            ])),
        )
        .expect("the two-instance host");
        host.activate_policy_catalog(&pending_policy_sources(1))
            .expect("the two-instance catalog");
        // B runs on A's host, so the two share its heavy-dispatch budget.
        let mut facts = pending_policy_facts();
        for instance in &mut facts.instances {
            instance.host_id = "fixture-host-a".to_owned();
        }
        let resources = policy_resources();
        let evaluate = |trigger| {
            host.evaluate_policy_cycle_with_test_inputs(
                &facts,
                &resources,
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                trigger,
            )
            .expect("a policy cycle")
        };

        let mut holder = TestClient::connect(&host);
        let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS_B);
        let cycle = evaluate(PolicyTrigger::FactsChanged);
        let evaluation = cycle.evaluation.as_ref().expect("the first evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS)
            .unwrap_or_else(|| panic!("A's dispatch intent: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("A's reason chain")
            .clone();
        record_policy_approval(&host, &intent);
        assert!(matches!(
            host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
                .expect("A's admission"),
            PolicyDispatchAdmission::Granted { .. }
        ));
        let release = holder.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            holder.send(&release).state(),
            RuntimeReceiptState::Completed
        );

        let cycle = evaluate(PolicyTrigger::Recovery);
        let evaluation = cycle.evaluation.as_ref().expect("the second evaluation");
        let decision = evaluation
            .decisions
            .iter()
            .find(|decision| {
                decision.task_id == "fixture.observe-b"
                    && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS_B)
            })
            .unwrap_or_else(|| panic!("B's decision: {evaluation:#?}"));
        let exhausted = decision
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == "heavy_scene_budget_exhausted");
        let b_intent = evaluation
            .dispatch_intents
            .iter()
            .any(|intent| intent.instance_id == POLICY_INSTANCE_ALIAS_B);
        assert_eq!(b_intent, b_dispatches, "{evaluation:#?}");
        assert_eq!(exhausted, !b_dispatches, "{decision:#?}");
        if !b_dispatches {
            assert_eq!(decision.state, SchedulingDecisionState::Blocked);
        }
        drop(holder);
        host.close().expect("close the host");
    }
}
