// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{CaptureBackendChoice, CaptureBackendName};
use actingcommand_lab::{
    CaptureBackendObservation, CaptureBackendReport, LabContainedPackageValidationResponse,
    LabValidateRequest, LabValidateResponse,
};
use serde::Serialize;
use std::path::PathBuf;

fn assert_serializable<T: Serialize>() {}

#[allow(dead_code)]
fn assert_methods_are_public<P: actingcommand_lab::LabPorts>(
    lab: &mut actingcommand_lab::Lab<P>,
    validate: LabValidateRequest,
) {
    let _: actingcommand_lab::LabResult<LabValidateResponse> = lab.lab_validate(validate);
}

#[test]
fn lab_validate_family_exposes_typed_requests_and_responses() {
    let _validate = LabValidateRequest {
        zip_path: PathBuf::from("bundle.zip"),
        expected_input_sha256: None,
    };
    assert_serializable::<LabValidateResponse>();
    assert_serializable::<LabContainedPackageValidationResponse>();
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
