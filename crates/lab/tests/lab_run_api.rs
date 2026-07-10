// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureBackendName, DeviceTarget,
    MaaTouchConfig, TouchBackendConfig,
};
use actingcommand_lab::{
    CaptureBackendObservation, CaptureBackendReport, FrameStoreControl, LabRunDeviceResolver,
    LabRunProcessContext, LabRunRequest, LabRunResponse, LabRunSelectedDevice, LabValidateRequest,
    LabValidateResponse, MemorySample, MemorySampleSource, RuntimeCommitSource,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

fn assert_serializable<T: Serialize>() {}

struct CompileDeviceResolver;

struct CompileCommitSource;

impl RuntimeCommitSource for CompileCommitSource {
    fn sample(&self) -> Option<String> {
        Some("deadbeef".to_string())
    }
}

impl LabRunDeviceResolver for CompileDeviceResolver {
    fn resolve_serial(
        &mut self,
        instance_id: &str,
    ) -> actingcommand_lab::LabResult<LabRunSelectedDevice> {
        Ok(LabRunSelectedDevice {
            id: instance_id.to_string(),
            serial: "fixture".to_string(),
        })
    }

    fn global_adb_provenance(&mut self) -> actingcommand_lab::LabResult<String> {
        Ok("adb".to_string())
    }

    fn capture_config(
        &mut self,
        _device: &LabRunSelectedDevice,
    ) -> actingcommand_lab::LabResult<CaptureBackendConfig> {
        Ok(CaptureBackendConfig::new(
            AdbConfig::default(),
            DeviceTarget::default(),
        ))
    }

    fn touch_config(
        &mut self,
        _device: &LabRunSelectedDevice,
    ) -> actingcommand_lab::LabResult<TouchBackendConfig> {
        Ok(TouchBackendConfig::new(
            AdbConfig::default(),
            DeviceTarget::default(),
            MaaTouchConfig::default(),
        ))
    }
}

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
    let process = LabRunProcessContext {
        current_dir: Some(PathBuf::from("workspace")),
        lease_root: PathBuf::from("locks"),
        os: "test".to_string(),
        app_version: "actinglab-test".to_string(),
        runtime_commit_source: Arc::new(CompileCommitSource),
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
        device_resolver: Box::new(CompileDeviceResolver),
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
