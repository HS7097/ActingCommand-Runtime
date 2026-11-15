// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #469 H3 (0.12.62; register §1.399; the design's place 8), the stage family the scripted
// emulator controller cannot reach (it gives every failure a device diagnostic): a provider
// control failure without a diagnostic is built as `emulator_control_failed` with the value
// `stage` at the default `mumu_manager_control` and no `device_category` (no device diagnostic
// names one; the primary detail keeps its default `child_exit`); the other three codes this
// function builds carry neither value. No device, no host: the function that builds the failure,
// on a hand-made one.

use super::*;
use actingcommand_device::{DeviceError, DeviceErrorCategory};

fn instance() -> InstanceId {
    *actingcommand_contract::IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// A provider control failure whose device error is `error`, after the vendor command exited 1.
fn failure(error: DeviceError) -> EmulatorControlFailure {
    EmulatorControlFailure {
        error,
        exit_code: Some(1),
        output_summary: "neutral vendor output".to_owned(),
        last_state: None,
        elapsed_ms: 2_000,
        device_process_present: None,
    }
}

#[test]
fn h3_a_control_failure_without_a_diagnostic_says_the_default_stage_and_no_category() {
    let (error, state, effect) = emulator_control_error(
        &failure(DeviceError::fatal("neutral control failure")),
        instance(),
        None,
    );
    assert_eq!(error.code(), HostCode::EmulatorControlFailed.as_str());
    assert_eq!(
        (state, effect),
        (
            RuntimeReceiptState::Failed,
            EffectDisposition::Indeterminate
        )
    );
    let outcome = error.record_outcome();
    assert_eq!(
        outcome.value("stage"),
        Some(&serde_json::json!(
            HostLocation::MumuManagerControl.location().as_str()
        ))
    );
    assert_eq!(outcome.value("device_category"), None, "{outcome:?}");
    assert_eq!(outcome.value("exit_code"), Some(&serde_json::json!(1)));
    let detail = error.diagnostic_detail().expect("the primary detail");
    assert_eq!(
        (
            detail.category(),
            detail.stage(),
            detail.backend(),
            detail.operation()
        ),
        (
            DeviceErrorCategory::ChildExit.as_str(),
            HostLocation::MumuManagerControl.location(),
            "mumu_manager",
            ContractLocation::ControlInstance.as_str()
        )
    );
}

#[test]
fn h3_the_other_codes_of_a_provider_control_failure_carry_no_stage_value() {
    for (stage, code) in [
        (
            DeviceStage::MumuManagerWaitTimeout.location(),
            HostCode::EmulatorControlWaitTimeout,
        ),
        (
            HostLocation::EmulatorControlBinding.location(),
            HostCode::EmulatorControlUnavailable,
        ),
        (
            ExecutionKernelLocation::EmulatorControlSupport.location(),
            HostCode::EmulatorControlUnsupported,
        ),
    ] {
        let (error, _, _) = emulator_control_error(
            &failure(
                DeviceError::fatal("neutral control failure")
                    .with_diagnostic(DeviceErrorCategory::Response, stage),
            ),
            instance(),
            None,
        );
        assert_eq!(error.code(), code.as_str());
        let outcome = error.record_outcome();
        assert_eq!(
            (outcome.value("stage"), outcome.value("device_category")),
            (None, None),
            "{outcome:?}"
        );
        let detail = error.diagnostic_detail().expect("the primary detail");
        assert_eq!(
            (detail.category(), detail.stage()),
            (DeviceErrorCategory::Response.as_str(), stage)
        );
    }
}
