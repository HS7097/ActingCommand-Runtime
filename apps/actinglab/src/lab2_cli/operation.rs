// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, LabOperationSelection, LabProjectionHint,
};

pub(super) fn run_contained_lab_do(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "do")?;
    let selection = selection(flags)?;
    let projection_hint = LabProjectionHint {
        sequence: flags
            .optional("--projection-sequence")
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    CliError::usage("--projection-sequence requires a positive integer")
                })
            })
            .transpose()?,
        content_sha256: flags.optional("--projection-hash"),
    };
    let instance = lab2_instance(global, flags);
    let logical_path = super::super::contained_resources::explicit_path(flags, "--zip")?;
    let expected = super::super::contained_resources::explicit_hash(flags)?;
    let reader = actingcommand_resource_tooling::open_published_package(&logical_path)?;
    let result = (|| {
        let path = reader
            .path()
            .canonicalize()
            .map_err(|_| CliError::package_invalid("package path could not be resolved"))?;
        let request = ContainedLabOperationRequest {
            package_path: path
                .to_str()
                .ok_or_else(|| CliError::package_invalid("package path is not UTF-8"))?
                .to_string(),
            expected_sha256: expected.hash().to_string(),
            selection,
            projection_hint,
        };
        request
            .validate()
            .map_err(|error| CliError::usage(error.code()))?;
        let session = begin_runtime_debug_session()?;
        let verified = session
            .run_contained_lab_operation(&instance, request)
            .map_err(|error| CliError::device(error.to_string()))?;
        let operation = verified.operation();
        let record = &operation.record;
        let prepared = &record.prepared;
        let frame_summary = |frame: &Option<actingcommand_contract::LabOperationFrame>,
                             projection: &Option<
            actingcommand_contract::ContainedPageObservation,
        >| {
            frame.as_ref().map(|frame| json!({
                "frame_id":frame.observation.artifact().frame_id,
                "frame_sha256":frame.observation.artifact().sha256,
                "frame_sequence":frame.verified.sequence,
                "lease_valid_after_capture":frame.lease_valid_after_capture,
                "page":projection.as_ref().map(|projection| &projection.projection.page),
                "status":projection.as_ref().map(|projection| projection.status),
                "projection_sequence":projection.as_ref().map(|projection| projection.projection_sequence),
            }))
        };
        let mut payload = json!({
            "req_id":verified.receipt().request_id(), "correlation_id":verified.receipt().correlation_id(),
            "state":if record.failure.is_none() { "completed" } else { "failed" },
            "instance":instance, "lease_id":prepared.lease_id, "action_id":record.input_action_id,
            "executed":match record.effect {
                EffectDisposition::Performed => Some(true),
                EffectDisposition::NotPerformed => Some(false),
                EffectDisposition::Indeterminate => None,
            }, "effect":record.effect,
            "actual_input":prepared.action, "actual_click":prepared.geometry,
            "before":frame_summary(&prepared.before_frame, &prepared.before_projection),
            "after":frame_summary(&record.after_frame, &record.after_projection),
            "device":{"authority":"runtime_execution_kernel"},
            "ledger":{"authority":"runtime_global_ledger", "prepared_sequence":record.prepared_artifact.verified.sequence,
                "input_sequence":record.input_event.map(|event| event.sequence),
                "terminal_sequence":operation.terminal_artifact.verified.sequence},
            "failure":record.failure, "cleanup_failure":record.cleanup_failure,
        });
        if global.verbose || flags.bool("--verbose") || flags.bool("--pretty") {
            payload["operation_record"] = json!(operation);
        }
        let evidence_id = payload["req_id"]
            .as_str()
            .ok_or_else(|| CliError::device("verified request ID is not canonical"))?
            .to_string();
        let mut projection_request = lab2_projection_request(flags, Some(evidence_id));
        for field in ["effect", "executed", "failure", "ledger"] {
            projection_request.fields.insert(field.to_string());
        }
        if global.verbose && projection_request.verbosity == ProjectionVerbosity::Min {
            projection_request.verbosity = ProjectionVerbosity::Normal;
        }
        let projected = project_record(&payload, &projection_request)
            .map_err(|error| CliError::device(error.to_string()))?;
        if let Some(failure) = &record.failure {
            let error = if failure.code == "lab_element_unavailable" {
                CliError::safety_blocked(
                    "capability_insufficient",
                    "the selected element is not resolvable in the current Runtime projection",
                    &[],
                )
            } else {
                CliError::device(format!(
                    "contained Lab operation failed at {:?}: {}",
                    failure.stage, failure.code
                ))
            };
            return Err(error.with_details(projected));
        }
        Ok(projected)
    })();
    // The logical publication's generation stays referenced throughout the RPC and validation.
    super::super::contained_resources::finish_package_use(result, reader.close())
}

fn selection(flags: &FlagArgs) -> CliOutcome<LabOperationSelection> {
    let tap = flags.values("--tap");
    let swipe = flags.values("--swipe");
    if flags.positionals.len() + tap.len() + swipe.len() != 1 {
        return Err(CliError::usage(
            "do --capture requires exactly one current <element-id>, --tap <x,y>, or --swipe <x1,y1,x2,y2,duration-ms>",
        ));
    }
    if let Some(id) = flags.positionals.first() {
        return Ok(LabOperationSelection::Element { id: id.clone() });
    }
    let (text, expected) = if let Some(tap) = tap.first() {
        (tap, 2)
    } else {
        (&swipe[0], 5)
    };
    let values = text.split(',').collect::<Vec<_>>();
    if values.len() != expected {
        return Err(CliError::usage(
            "Lab coordinates must be comma-separated integers",
        ));
    }
    let coordinate = |index: usize| {
        values[index]
            .parse::<i32>()
            .map_err(|_| CliError::usage("Lab coordinates must be i32 integers"))
    };
    let action = if expected == 2 {
        InputAction::Tap {
            x: coordinate(0)?,
            y: coordinate(1)?,
        }
    } else {
        InputAction::Swipe {
            x1: coordinate(0)?,
            y1: coordinate(1)?,
            x2: coordinate(2)?,
            y2: coordinate(3)?,
            duration_ms: values[4].parse::<u64>().map_err(|_| {
                CliError::usage("swipe duration must be a positive millisecond integer")
            })?,
        }
    };
    Ok(LabOperationSelection::Coordinates { action })
}
