// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, LabArrivalCondition, LabOperationSelection, LabProjectionHint,
};
use actingcommand_lab::{
    ClickEffect, ClickPlan, CommitClickRequest, OpaqueJson, PlanClickRequest, RecordPoint,
    RecordingLock, record_commit_click, record_plan_click,
};

pub(super) fn run_contained_lab_do(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    reject_mixed_online_and_offline_scene(flags, "do")?;
    // `do --capture --record`: the lock is held across the device click; the point comes from
    // the open step's rectangle, planned before anything is pressed.
    let recording = if flags.bool("--record") {
        Some(begin_record_click(global, flags)?)
    } else {
        None
    };
    let selection = match &recording {
        Some((_, plan)) => LabOperationSelection::Coordinates {
            action: InputAction::Tap {
                x: plan.point.x,
                y: plan.point.y,
            },
        },
        None => selection(flags)?,
    };
    let after = flags
        .optional("--after-page")
        .map(|page_id| {
            let timeout_ms = flags
                .optional("--after-timeout-ms")
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| CliError::usage("--after-timeout-ms requires an integer"))
                })
                .transpose()?
                .unwrap_or(5_000);
            Ok::<_, CliError>(LabArrivalCondition {
                page_id,
                timeout_ms,
            })
        })
        .transpose()?;
    if after.is_none() && flags.optional("--after-timeout-ms").is_some() {
        return Err(CliError::usage("--after-timeout-ms requires --after-page"));
    }
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
    let reader = super::super::contained_resources::PackageInput::open(flags)?;
    let result = (|| {
        let path = reader.path();
        let request = ContainedLabOperationRequest {
            package_path: path
                .to_str()
                .ok_or_else(|| CliError::package_invalid("package path is not UTF-8"))?
                .to_string(),
            expected_sha256: reader.reference.clone(),
            selection,
            projection_hint,
            after,
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
        if prepared.after.is_some() {
            payload["after_condition"] = json!(prepared.after);
            payload["arrival"] = json!(record.arrival);
        }
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
        if prepared.after.is_some() {
            projection_request
                .fields
                .insert("after_condition".to_string());
            projection_request.fields.insert("arrival".to_string());
        }
        if global.verbose && projection_request.verbosity == ProjectionVerbosity::Min {
            projection_request.verbosity = ProjectionVerbosity::Normal;
        }
        let projected = project_record(&payload, &projection_request)
            .map_err(|error| CliError::device(error.to_string()))?;
        if let Some((lock, plan)) = &recording {
            if !matches!(record.effect, EffectDisposition::NotPerformed) {
                return commit_record_click(lock, plan, record, &payload, projected);
            }
            if record.failure.is_none() {
                return Err(CliError::device(
                    "do --capture --record: the Runtime reported the click as not performed \
                     without a failure; nothing was recorded",
                )
                .with_details(projected));
            }
        }
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

/// Plans the recorded click: the instance check, the recording lock and the point inside
/// the open step's rectangle (`--tap-rect` declares it, `--tap` picks the point).
fn begin_record_click(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<(RecordingLock, ClickPlan)> {
    let instance = lab2_instance(global, flags);
    let tap_rect = match flags.values("--tap-rect").as_slice() {
        [] => None,
        [value] => Some(crate::commands::parse_record_mark_rect(
            value,
            "--tap-rect",
        )?),
        _ => return Err(CliError::usage("--tap-rect may be given once")),
    };
    let tap = match flags.values("--tap").as_slice() {
        [] => None,
        [value] => {
            let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
            let [x, y] = parts.as_slice() else {
                return Err(CliError::usage(format!("--tap must be x,y, got {value}")));
            };
            let coordinate = |text: &str| {
                text.parse::<i32>()
                    .map_err(|_| CliError::usage("Lab coordinates must be i32 integers"))
            };
            Some(RecordPoint {
                x: coordinate(*x)?,
                y: coordinate(*y)?,
            })
        }
        _ => return Err(CliError::usage("--tap may be given once")),
    };
    let lock =
        crate::commands::record_flag_begin(global, flags, &instance, "do --capture --record")?;
    let plan = record_plan_click(&lock, &PlanClickRequest { tap, tap_rect })?;
    Ok((lock, plan))
}

/// Records the Runtime outcome of the planned click on its step; the operation summary
/// stays in the output and in any error.
fn commit_record_click(
    lock: &RecordingLock,
    plan: &ClickPlan,
    record: &actingcommand_contract::LabOperationRecord,
    payload: &Value,
    mut projected: Value,
) -> CliOutcome<Value> {
    let opaque = |value: Option<Value>| -> CliOutcome<Option<OpaqueJson>> {
        value
            .filter(|value| !value.is_null())
            .map(|value| OpaqueJson::from_serializable(&value))
            .transpose()
    };
    let informational = |field: &str| -> Option<Value> {
        payload.get(field).cloned().map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                object.insert("informational".to_string(), Value::Bool(true));
            }
            value
        })
    };
    let prepared = &record.prepared;
    let carrier = json!({
        "package_ref": prepared.expected_package_sha256,
        "actual": prepared.actual_package_sha256
    });
    let request = CommitClickRequest {
        effect: if matches!(record.effect, EffectDisposition::Performed) {
            ClickEffect::Performed
        } else {
            ClickEffect::Indeterminate
        },
        has_failure: record.failure.is_some(),
        carrier_package: Some(OpaqueJson::from_serializable(&carrier)?),
        req_id: opaque(payload.get("req_id").cloned())?,
        correlation_id: opaque(payload.get("correlation_id").cloned())?,
        action_id: opaque(payload.get("action_id").cloned())?,
        lease_id: opaque(payload.get("lease_id").cloned())?,
        failure: record
            .failure
            .as_ref()
            .map(OpaqueJson::from_serializable)
            .transpose()?,
        before: opaque(informational("before"))?,
        after: opaque(informational("after"))?,
    };
    match record_commit_click(lock, plan, &request) {
        Ok(outcome) => {
            projected["record"] = serde_json::to_value(outcome).map_err(|error| {
                CliError::usage(format!("failed to encode the record: {error}"))
            })?;
            Ok(projected)
        }
        Err(mut error) => {
            let mut details = error.details.take().unwrap_or_else(|| json!({}));
            if let Some(object) = details.as_object_mut() {
                object.insert("operation".to_string(), projected);
            }
            Err(error.with_details(details))
        }
    }
}
