// SPDX-License-Identifier: AGPL-3.0-only

//! #503 NOISE-A (0.12.102; register §1.552): the monitor's half. A heavy process the sampler
//! counts as owned raises no `third_party` pressure and leaves the pacing as a calm host's, where
//! a heavy third-party process still starts an elevated pressure and the stage-one floor; the
//! images the host hands the sampler are each registered instance's debug bridges and, for an
//! emulator instance, the images the device layer names for its installation; a
//! `task.effect_completed` whose action has no pending intent, an application step's, is neither
//! measured nor a failure, while an unpaired capture completion and an intent of another instance
//! are judged as before. Scripted samples at given times; no clock is read.

use super::*;
use actingcommand_contract::{
    EffectiveDeviceConfiguration, IdentifierIssuer, InstanceId, PerformancePacingStage,
};
use actingcommand_execution_kernel::{
    DiscoveredInstanceBinding, HandsetEndpoint, PendingAdbEndpoint, ResolvedAdbEndpoint,
};
use actingcommand_host_metrics::ProcessMetricCoverage;
use actingcommand_runtime_config::EffectiveConfig;

const INSTANCE: &str = "fixture-instance";
/// An owned process, as the sampler counts a process at a handed-over image.
const OWNED_PID: u32 = 4_242;
const THIRD_PARTY_PID: u32 = 5_151;

/// The samples a test scripts, one per tick.
struct ScriptedSamples(VecDeque<HostSample>);

impl HostSampler for ScriptedSamples {
    fn sample(
        &mut self,
        _observed_at_unix_ms: u64,
        _owned_processes: &BTreeMap<u32, String>,
        _top_process_count: usize,
        _thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, Code> {
        Ok(self.0.pop_front().expect("the test scripts every sample"))
    }
}

fn scripted(samples: Vec<HostSample>) -> PerformanceMonitor {
    PerformanceMonitor::enabled(
        PerformanceMonitorConfig::default(),
        Ok(Box::new(ScriptedSamples(samples.into()))),
    )
    .expect("monitor")
}

fn process_of(pid: u32, owner: HostProcessOwnership, cpu_basis_points: u16) -> HostProcessSample {
    HostProcessSample {
        pid,
        process_name: format!("process-{pid}"),
        ownership: owner,
        cpu_basis_points,
        working_set_bytes: 100,
        peak_working_set_bytes: None,
        process_created_at_windows_100ns: None,
        io_bytes_per_second: 0,
    }
}

/// A calm host at `at` (no whole-machine pressure), with these owned and third-party lists.
fn sample_at(
    at: u64,
    owned_processes: Vec<HostProcessSample>,
    third_party_high_load: Vec<HostProcessSample>,
) -> HostSample {
    HostSample {
        observed_at_unix_ms: at,
        cpu_total_basis_points: 2_000,
        cpu_per_core_basis_points: vec![2_000],
        ram_used_basis_points: 4_000,
        disk_queue_depth_milli: Some(500),
        disk_latency_micros: Some(1_000),
        gpu_basis_points: None,
        foreground: None,
        owned_processes,
        third_party_high_load,
        process_coverage: ProcessMetricCoverage {
            enumerated_processes: 3,
            sampled_processes: 3,
            audited_protected_processes_excluded: 0,
            unconfirmed_access_denied_processes: 0,
            unexpectedly_inaccessible_processes: 0,
            readable_basis_points: 10_000,
        },
        unavailable_metrics: vec![HostMetric::Gpu, HostMetric::ForegroundProcess],
    }
}

fn runtime_process() -> HostProcessSample {
    process_of(std::process::id(), HostProcessOwnership::Runtime, 100)
}

fn pressures_started(events: &[PerformanceSemanticEvent]) -> Vec<PerformancePressureRecord> {
    events
        .iter()
        .filter_map(|event| match event {
            PerformanceSemanticEvent::PressureStarted(data) => Some(data.pressure.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn noise_a_a_heavy_owned_process_raises_no_pressure_and_paces_as_a_calm_host() {
    let settings = EffectiveConfig::default()
        .performance_measurement()
        .expect("the shipped performance section");
    let instances = [INSTANCE.to_owned()];
    let owned_heavy = |at| {
        sample_at(
            at,
            vec![
                runtime_process(),
                process_of(OWNED_PID, HostProcessOwnership::Owned, 9_000),
            ],
            Vec::new(),
        )
    };
    // Three samples whose only heavy process is owned, then one in which it has exited.
    let mut owned = scripted(vec![
        owned_heavy(2_000),
        owned_heavy(4_000),
        owned_heavy(6_000),
        sample_at(8_000, vec![runtime_process()], Vec::new()),
    ]);
    let mut calm = scripted(
        [2_000, 4_000, 6_000]
            .map(|at| sample_at(at, vec![runtime_process()], Vec::new()))
            .into(),
    );
    let heavy_third_party = |at| {
        sample_at(
            at,
            vec![runtime_process()],
            vec![process_of(
                THIRD_PARTY_PID,
                HostProcessOwnership::ThirdParty,
                5_000,
            )],
        )
    };
    let mut third_party = scripted(vec![
        heavy_third_party(2_000),
        heavy_third_party(4_000),
        heavy_third_party(6_000),
    ]);

    let mut third_party_started = Vec::new();
    for at in [2_000, 4_000, 6_000] {
        let events = owned.tick(at).expect("an owned sample").events;
        assert!(pressures_started(&events).is_empty(), "{events:?}");
        calm.tick(at).expect("a calm sample");
        third_party_started.extend(pressures_started(
            &third_party.tick(at).expect("a third-party sample").events,
        ));
    }

    assert_eq!(
        owned
            .control_observation(6_000)
            .expect("the owned observation")
            .expect("a measured observation")
            .third_party_pressure_basis_points,
        Some(0)
    );
    // The owned process exits: the process metrics stay available and nothing degrades.
    let after_exit = owned.tick(8_000).expect("the sample after the exit");
    assert!(
        !after_exit
            .events
            .iter()
            .any(|event| matches!(event, PerformanceSemanticEvent::MonitorDegraded(_))),
        "{:?}",
        after_exit.events
    );
    assert_eq!(
        owned
            .control_observation(8_000)
            .expect("the observation after the exit")
            .expect("a measured observation")
            .third_party_pressure_basis_points,
        Some(0)
    );
    // The pacing under the owned process is a calm host's: S0, factor 1 000, no extra round.
    owned
        .evaluate_pacing(8_000, &settings, &instances)
        .expect("pacing under the owned process");
    calm.evaluate_pacing(8_000, &settings, &instances)
        .expect("pacing of the calm host");
    assert_eq!(owned.effective_stage(INSTANCE), PerformancePacingStage::S0);
    assert_eq!(owned.effective_pacing(INSTANCE), EffectivePacing::UNPACED);
    assert_eq!(
        owned.effective_pacing(INSTANCE),
        calm.effective_pacing(INSTANCE)
    );

    // A heavy unlisted process still starts an elevated third-party pressure, and the floor.
    assert_eq!(third_party_started.len(), 1, "{third_party_started:?}");
    assert_eq!(
        third_party_started[0].kind,
        PerformancePressureKind::ThirdParty
    );
    assert_eq!(
        third_party_started[0].severity,
        PerformancePressureSeverity::Elevated
    );
    third_party
        .evaluate_pacing(8_000, &settings, &instances)
        .expect("pacing under the third-party process");
    assert_eq!(
        third_party.effective_stage(INSTANCE),
        PerformancePacingStage::S1
    );
    assert_eq!(
        third_party.effective_pacing(INSTANCE),
        EffectivePacing {
            factor_milli: 1_250,
            extra_wait_rounds: 1,
        }
    );
}

fn observation(
    event_type: EventType,
    instance_id: &str,
    observed_at_unix_ms: u64,
    frame_id: Option<FrameId>,
    action_id: Option<ActionId>,
) -> PipelineEventObservation {
    PipelineEventObservation {
        event_type,
        instance_id: instance_id.to_owned(),
        observed_at_unix_ms,
        frame_id,
        recognition_id: None,
        action_id,
        touch_response_us: None,
        capture_acquire_us: None,
    }
}

#[test]
fn noise_a_an_application_steps_completion_keeps_the_monitor_healthy_and_a_capture_one_degrades() {
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let never_intended = *identifiers.mint_action_id().expect("action").transport();
    let mut monitor = scripted(vec![sample_at(1_000, vec![runtime_process()], Vec::new())]);
    monitor.tick(1_000).expect("a host sample");
    let health = monitor.health;

    let events = monitor
        .observe_pipeline_event(observation(
            EventType::TaskEffectCompleted,
            INSTANCE,
            2_000,
            None,
            Some(never_intended),
        ))
        .expect("an effect without an input");
    assert!(events.is_empty(), "{events:?}");
    assert!(
        monitor
            .record_pipeline_success(2_000)
            .expect("the accepted event")
            .is_empty()
    );
    assert_eq!(monitor.consecutive_pipeline_failures, 0);
    assert_eq!(monitor.health, health);
    assert!(monitor.pipeline_samples.is_empty());
    assert_eq!(
        monitor
            .context(INSTANCE, 2_000)
            .expect("the context")
            .max_action_effect_latency_ms,
        None
    );

    // An intent of another instance is judged as before.
    let intended = *identifiers.mint_action_id().expect("action").transport();
    monitor
        .observe_pipeline_event(observation(
            EventType::TaskEffectIntent,
            "fixture-instance-b",
            2_100,
            None,
            Some(intended),
        ))
        .expect("the other instance's intent");
    let conflict = monitor
        .observe_pipeline_event(observation(
            EventType::TaskEffectCompleted,
            INSTANCE,
            2_200,
            None,
            Some(intended),
        ))
        .expect_err("a completion of another instance's intent");
    assert_eq!(conflict.code(), "performance_pipeline_identity_conflict");

    // A capture completion without its request still degrades the monitor.
    let frame_id = *identifiers.mint_frame_id().expect("frame").transport();
    let missing = monitor
        .observe_pipeline_event(observation(
            EventType::CaptureCompleted,
            INSTANCE,
            2_300,
            Some(frame_id),
            None,
        ))
        .expect_err("a capture completion without its request");
    assert_eq!(missing.code(), "performance_pipeline_start_missing");
    let degraded = monitor
        .record_monitor_failure(2_300, missing.outcome(), None)
        .expect("the degraded monitor");
    assert!(matches!(
        degraded.events.as_slice(),
        [PerformanceSemanticEvent::MonitorDegraded(data)]
            if data.outcome.as_ref().map(|outcome| outcome.code().as_str())
                == Some("performance_pipeline_start_missing")
    ));
}

fn instance_identity() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifiers")
        .mint_instance_id()
        .expect("instance")
        .transport()
}

/// A registration's configuration whose input and capture use `adb`, with a configured root.
fn configuration_of(adb: &str, configured_root: Option<&str>) -> EffectiveDeviceConfiguration {
    EffectiveDeviceConfiguration {
        input_backend: "adb".to_owned(),
        capture_backend: "adb".to_owned(),
        input_adb: adb.to_owned(),
        capture_adb: adb.to_owned(),
        configured_serial: None,
        resolved_serial: "127.0.0.1:16384".to_owned(),
        input_command_timeout_ms: 12_000,
        capture_command_timeout_ms: 12_000,
        capture_timeout_ms: 12_000,
        configured_mumu_root: configured_root.map(std::path::PathBuf::from),
        configured_capture_dll: None,
    }
}

#[test]
fn noise_a_the_host_hands_over_each_instances_bridges_and_its_installations_images() {
    // A discovery-bound emulator instance, stopped: its bridge under the installation's
    // `nx_main`, its root from the vendor tool it was discovered through.
    let discovered = ResolvedExecutionInstance::new(instance_identity(), "pending")
        .with_configuration(configuration_of(r"C:\Fixture\Emu\nx_main\adb.exe", None))
        .with_pending_endpoint(PendingAdbEndpoint::new(
            "127.0.0.1",
            DiscoveredInstanceBinding::new(
                0,
                "fixture",
                "5.0.0",
                r"\\?\C:\Fixture\Emu\nx_main\MuMuManager.exe",
            ),
        ));
    // An emulator instance with a configured root and a bridge outside every root.
    let configured = ResolvedExecutionInstance::new(instance_identity(), "127.0.0.1:16416")
        .with_configuration(configuration_of(
            r"C:\Fixture\Tools\adb.exe",
            Some(r"C:\Fixture\Second"),
        ))
        .with_adb_endpoint(ResolvedAdbEndpoint::new("127.0.0.1", 16_416, false));
    // A handset: its bridge only, though the bridge lies under an installation.
    let handset = ResolvedExecutionInstance::new(instance_identity(), "handset")
        .with_configuration(configuration_of(r"C:\Fixture\Third\nx_main\adb.exe", None))
        .with_adb_endpoint(
            ResolvedAdbEndpoint::new("127.0.0.1", 5_555, true)
                .with_handset(HandsetEndpoint::Unbound),
        );
    // A fixture simulation runs nothing.
    let fixture = ResolvedExecutionInstance::fixture_simulation(instance_identity())
        .with_configuration(configuration_of(r"C:\Fixture\Fourth\nx_main\adb.exe", None));

    let images = owned_process_images(&[discovered, configured, handset.clone(), fixture.clone()]);
    let owns = |name: &str, path: &str| images.owns(name, || Some(path.to_owned()));

    // Each registered device instance's bridges.
    assert!(owns("adb.exe", r"C:\Fixture\Emu\nx_main\adb.exe"));
    assert!(owns("adb.exe", r"C:\Fixture\Tools\adb.exe"));
    assert!(owns("adb.exe", r"C:\Fixture\Third\nx_main\adb.exe"));
    assert!(!owns("adb.exe", r"C:\Fixture\Fourth\nx_main\adb.exe"));
    assert!(!owns("adb.exe", r"C:\Elsewhere\adb.exe"));
    // The installations' front end and the device process of any version.
    for root in [r"C:\Fixture\Emu", r"C:\Fixture\Second"] {
        assert!(owns(
            "MuMuNxMain.exe",
            &format!(r"{root}\nx_main\MuMuNxMain.exe")
        ));
        assert!(owns(
            "MuMuNxDevice.exe",
            &format!(r"{root}\nx_device\17.0\shell\MuMuNxDevice.exe")
        ));
        assert!(owns(
            "mumunxdevice.exe",
            &format!(r"{root}\NX_DEVICE\9.9\Shell\MuMuNxDevice.exe")
        ));
        assert!(!owns(
            "MuMuNxDevice.exe",
            &format!(r"{root}\nx_device\shell\MuMuNxDevice.exe")
        ));
    }
    // Neither the handset's installation nor the fixture's, nor any other.
    for root in [r"C:\Fixture\Third", r"C:\Fixture\Fourth", r"C:\Other"] {
        assert!(!owns(
            "MuMuNxMain.exe",
            &format!(r"{root}\nx_main\MuMuNxMain.exe")
        ));
        assert!(!owns(
            "MuMuNxDevice.exe",
            &format!(r"{root}\nx_device\17.0\shell\MuMuNxDevice.exe")
        ));
    }
    // The vendor's virtual machine images, by their name.
    assert!(images.owns("MuMuVMMHeadless.exe", || None));
    assert!(images.owns("MuMuNxSVC.exe", || None));

    // Without an emulator instance whose installation is known no vendor image is owned.
    let devices_only = owned_process_images(&[handset, fixture.clone()]);
    assert!(!devices_only.owns("MuMuVMMHeadless.exe", || None));
    assert!(!devices_only.owns("MuMuNxMain.exe", || Some(
        r"C:\Fixture\Third\nx_main\MuMuNxMain.exe".to_owned()
    )));
    assert!(owned_process_images(&[fixture]).is_empty());
}
