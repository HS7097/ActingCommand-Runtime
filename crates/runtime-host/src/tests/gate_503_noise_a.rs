// SPDX-License-Identifier: AGPL-3.0-only

//! #503 NOISE-A (0.12.102; register §1.552), the host's half: an application step writes
//! `task.effect_completed` with no `task.effect_intent` (`contracts/application-lifecycle.md`),
//! and the pipeline monitor takes that completion for no failed pairing, so the run writes no
//! `perf.monitor_degraded` and no `perf.monitor_recovered` (until 0.12.102 it wrote one of each).
//! One fake device instance, the performance monitor on with a scripted sampler of calm samples,
//! a person's run of a package whose one step restarts the instance's application to `home`,
//! which its recognition pack knows by pixel 0 of the fake device's first frame. No clock is read
//! and nothing sleeps here.

use super::*;
use actingcommand_host_metrics::{
    HostMetric, HostSample, HostSampler, ProcessLoadThresholds, ProcessMetricCoverage,
};
use serde_json::{Value, json};

const ALIAS: &str = "neutral.instance";
/// The fake device's first pixel: the package's home.
const HOME: [u8; 3] = [255, 0, 0];

/// Every sample calm, at the time the monitor asks for, so no sampler failure is recorded.
struct CalmSampler;

impl HostSampler for CalmSampler {
    fn sample(
        &mut self,
        observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, actingcommand_contract::outcome::Code> {
        Ok(HostSample {
            observed_at_unix_ms,
            cpu_total_basis_points: 1_000,
            cpu_per_core_basis_points: vec![1_000],
            ram_used_basis_points: 4_000,
            disk_queue_depth_milli: Some(500),
            disk_latency_micros: Some(1_000),
            gpu_basis_points: None,
            foreground: None,
            owned_processes: Vec::new(),
            third_party_high_load: Vec::new(),
            process_coverage: ProcessMetricCoverage {
                enumerated_processes: 1,
                sampled_processes: 1,
                audited_protected_processes_excluded: 0,
                unconfirmed_access_denied_processes: 0,
                unexpectedly_inaccessible_processes: 0,
                readable_basis_points: 10_000,
            },
            unavailable_metrics: vec![HostMetric::Gpu, HostMetric::ForegroundProcess],
        })
    }
}

/// The application entry package: from any screen its one step restarts the instance's
/// application, to `home`.
fn application_step_package(root: &TempDir) -> PathBuf {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.application-step",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "entry_page": "any",
        "target_page": "home",
        "operations": [
            {"id": "start", "from": "any", "to": "home", "application": {"action": "restart"}}
        ]
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [{"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME}]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []}
    ]});
    let files: [(&str, Value); 6] = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    let path = root.path().join("application-step.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner())
        .expect("write the application step package");
    path
}

#[test]
fn noise_a_an_application_step_writes_no_degraded_or_recovered_monitor_record() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root).with_performance_monitor(PerformanceMonitorConfig::default()),
        Arc::new(FakeProvider::one(ALIAS, instance_id(), Arc::clone(&state))),
    )
    .expect("runtime host");
    host.replace_capacity_sampler_for_test(Box::new(CalmSampler))
        .expect("the calm sampler");
    let package = application_step_package(&root);

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::run_contained_task(
                    ALIAS,
                    ids.mint_holder_id().expect("holder"),
                    ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
                ),
            ),
            ConnectionId::new(5_031).expect("connection"),
        )
        .expect("the run's receipt");

    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                ..
            })
        ),
        "the run completes: {receipt:?}"
    );
    assert_eq!(state.application_count.load(Ordering::Acquire), 1);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let count = |event_type: EventType| {
        events
            .iter()
            .filter(|event| event.event_type() == event_type)
            .count()
    };
    // The premise: the application step's completion, with no intent.
    assert_eq!(count(EventType::TaskEffectCompleted), 1);
    assert_eq!(count(EventType::TaskEffectIntent), 0);
    assert_eq!(count(EventType::PerformanceMonitorDegraded), 0);
    assert_eq!(count(EventType::PerformanceMonitorRecovered), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
