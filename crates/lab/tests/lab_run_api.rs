// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName, DeviceTarget,
    MaaTouchConfig, TouchBackendConfig,
};
use actingcommand_lab::{
    CaptureBackendObservation, CaptureBackendReport, FrameStoreControl, LabRunDeviceCandidate,
    LabRunDeviceConfig, LabRunProcessContext, LabRunRequest, LabRunResponse, LabValidateRequest,
    LabValidateResponse, MemorySample, MemorySampleSource,
};
use serde::Serialize;
use std::path::PathBuf;

fn assert_serializable<T: Serialize>() {}

#[allow(dead_code)]
fn assert_methods_are_public<P: actingcommand_lab::LabPorts>(
    lab: &mut actingcommand_lab::Lab<P>,
    run: LabRunRequest,
    validate: LabValidateRequest,
) {
    let _: actingcommand_lab::LabResult<LabRunResponse> = lab.lab_run(run);
    let _: actingcommand_lab::LabResult<LabValidateResponse> = lab.lab_validate(validate);
}

#[test]
fn lab_run_family_exposes_typed_requests_and_responses() {
    let adb = AdbConfig::default();
    let target = DeviceTarget::default();
    let candidate = LabRunDeviceCandidate::resolved(
        "fixture",
        LabRunDeviceConfig {
            instance: target.resolved_serial(),
            adb_path: adb.adb_path.clone(),
            capture_config: CaptureBackendConfig::new(adb.clone(), target.clone()),
            touch_config: TouchBackendConfig::new(adb, target, MaaTouchConfig::default()),
        },
    );
    let process = LabRunProcessContext {
        current_dir: Some(PathBuf::from("workspace")),
        lease_root: PathBuf::from("locks"),
        os: "test".to_string(),
        runtime_commit: Some("deadbeef".to_string()),
        memory_source: MemorySampleSource::fixed(MemorySample {
            total_bytes: 8 * 1024 * 1024 * 1024,
            available_bytes: 4 * 1024 * 1024 * 1024,
        }),
    };
    let _run = LabRunRequest {
        zip_path: PathBuf::from("bundle.zip"),
        out_path: PathBuf::from("result.zip"),
        run_root: PathBuf::from("runs"),
        game: None,
        server: None,
        instance: None,
        device_candidates: vec![candidate],
        capture_interval_override: None,
        capture_backend_override: Some(CaptureBackendChoice::Adb),
        frame_store_override: FrameStoreControl::default(),
        expected_input_sha256: None,
        process,
    };
    let _validate = LabValidateRequest {
        zip_path: PathBuf::from("bundle.zip"),
        expected_input_sha256: None,
    };

    assert_serializable::<LabRunResponse>();
    assert_serializable::<LabValidateResponse>();
}

#[test]
fn capture_factory_can_publish_typed_selection_diagnostics() {
    let observation = CaptureBackendObservation::default();
    assert!(observation.snapshot().is_err());

    observation
        .record(CaptureBackendReport {
            requested: CaptureBackendChoice::Auto,
            used: CaptureBackendName::AdbScreencap,
            attempts: Vec::new(),
        })
        .expect("record capture report");

    let report = observation.snapshot().expect("capture report");
    assert_eq!(report.used, CaptureBackendName::AdbScreencap);
}
