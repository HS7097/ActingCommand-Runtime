// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::codes::{RuntimeContractCode, SharedCode};
use actingcommand_contract::{
    ContainedLabOperationRequest, LabArrivalCondition, LabOperationFailure, LabOperationSelection,
    LabOperationStage, LabProjectionHint,
};
use actingcommand_lab::{
    ClickEffect, ClickGesture, ClickPlan, CommitClickRequest, OpaqueJson, PlanClickRequest,
    PlannedSwipe, RecordPoint, RecordingLock, record_commit_click, record_plan_click,
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
        Some((_, plan)) => recorded_click_selection(plan)?,
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
            selection,
            projection_hint,
            after,
        };
        request
            .validate()
            .map_err(|error| CliError::usage(error.code().as_str()))?;
        // #411 Lab L5: the frames the package's target consensus makes each of the operation's
        // observations take.
        let frames = super::super::contained_resources::online_frame_plan(&instance, path)?;
        let session = begin_runtime_debug_session()?;
        let verified = session
            .run_contained_lab_operation(&instance, request, frames)
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
        // Workflow #410 item 1: the coordinate system of the frame the action's coordinates
        // were chosen on (the before frame).
        if let Some(before) = &prepared.before_frame {
            payload["frame_coordinates"] =
                super::frame::coordinates(before.observation.width(), before.observation.height());
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
        let mut projected = project_record(&payload, &projection_request)
            .map_err(|error| CliError::device(error.to_string()))?;
        // Workflow #410 item 1: after an action, its result by default: the after page's
        // projection when the page is known, otherwise the after frame's path and preview path.
        // It joins the answer after the Min projection of the operation members, so they keep
        // their budget, and it is in the details of a failure too.
        if let Some(attachment) = super::frame::after_attachment(
            record,
            flags.bool("--preview"),
            projection_request.verbosity == ProjectionVerbosity::Min,
        )? {
            projected["after_attachment"] = attachment;
        }
        // #411 A19: in every answer and every failure's details, `arrival` is the last member.
        let projected = arrival_last(projected);
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
            return Err(operation_failure_error(failure, projected));
        }
        Ok(projected)
    })();
    // #411 A19: `arrival` stays the last member when a `--record` answer gains its `record`.
    let result = result.map(arrival_last);
    // The logical publication's generation stays referenced throughout the RPC and validation.
    super::super::contained_resources::finish_package_use(result, reader.close())
}

/// #411 A19: the error a failed online `do` answers, its details the operation's answer. A timed
/// out arrival is "the page did not arrive", as a navigation step whose page did not arrive
/// answers it: `lab_arrival_timeout`, exit 3, blocked by the arrival page
/// (contained-lab-operation.md). An unresolvable element stays `capability_insufficient`, and any
/// other failure a device error.
fn operation_failure_error(failure: &LabOperationFailure, details: Value) -> CliError {
    let error = if failure.stage == LabOperationStage::Arrival
        && failure.code == SharedCode::LabArrivalTimeout.as_str()
    {
        CliError::safety_blocked(
            SharedCode::LabArrivalTimeout.as_str(),
            "the action was performed, but no frame sampled after it matched the requested page \
             before the arrival condition ended",
            &["arrival_page"],
        )
    } else if failure.code == RuntimeContractCode::LabElementUnavailable.as_str() {
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
    error.with_details(details)
}

/// #411 A19: an online `do` answer with its `arrival` as the last member, every other member in
/// its order and every value unchanged, so a reader that cuts the text (MCP's `raw_text`) loses
/// the arrival's samples first. An answer without `arrival` is returned as it is.
fn arrival_last(mut answer: Value) -> Value {
    if let Some(object) = answer.as_object_mut()
        && let Some(arrival) = object.shift_remove("arrival")
    {
        object.insert("arrival".to_string(), arrival);
    }
    answer
}

/// The input `do --capture --record` presses for the open step's planned click, as the kernel
/// presses the click the recording generates for it: a tap at its point; a drag (Workflow #390
/// LAB-2) as one swipe from its point to its end point over its duration; a long press (#411 Lab
/// L3) as a long tap at its point over its duration.
fn recorded_click_selection(plan: &ClickPlan) -> CliOutcome<LabOperationSelection> {
    let action = match (plan.gesture, plan.to_point) {
        (None, _) => InputAction::Tap {
            x: plan.point.x,
            y: plan.point.y,
        },
        (Some(ClickGesture::LongPress { duration_ms }), _) => InputAction::LongTap {
            x: plan.point.x,
            y: plan.point.y,
            duration_ms,
        },
        (Some(ClickGesture::Drag { duration_ms, .. }), Some(to)) => InputAction::Swipe {
            x1: plan.point.x,
            y1: plan.point.y,
            x2: to.x,
            y2: to.y,
            duration_ms,
        },
        (Some(ClickGesture::Drag { .. }), None) => {
            return Err(CliError::usage(
                "do --capture --record: the planned drag of the open step names no end point",
            ));
        }
        // #411 Lab BD: the recording refuses to plan a brake drag (`record_plan_click`), so no
        // plan carries one; a Lab operation has no input for it.
        (Some(gesture @ ClickGesture::SingleTouchDragWithVerticalBrakeV1 { .. }), _) => {
            return Err(CliError::safety_blocked(
                actingcommand_lab::LabCode::RecordGestureExecutionUnsupported.as_str(),
                "do --capture --record: a Lab operation does not press the brake drag of the open \
                 step",
                &["session_record"],
            )
            .with_details(json!({"step": plan.step, "gesture": gesture})));
        }
    };
    Ok(LabOperationSelection::Coordinates { action })
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
                x: coordinate(x)?,
                y: coordinate(y)?,
            })
        }
        _ => return Err(CliError::usage("--tap may be given once")),
    };
    // Workflow #390 LAB-2: `--swipe x1,y1,x2,y2,ms` executes the open step's declared drag.
    let swipe = match flags.values("--swipe").as_slice() {
        [] => None,
        [value] => {
            let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
            let [x1, y1, x2, y2, duration] = parts.as_slice() else {
                return Err(CliError::usage(format!(
                    "--swipe must be x1,y1,x2,y2,duration-ms, got {value}"
                )));
            };
            let coordinate = |text: &str| {
                text.parse::<i32>()
                    .map_err(|_| CliError::usage("Lab coordinates must be i32 integers"))
            };
            Some(PlannedSwipe {
                from: RecordPoint {
                    x: coordinate(x1)?,
                    y: coordinate(y1)?,
                },
                to: RecordPoint {
                    x: coordinate(x2)?,
                    y: coordinate(y2)?,
                },
                duration_ms: duration.parse::<u64>().map_err(|_| {
                    CliError::usage("swipe duration must be a positive millisecond integer")
                })?,
            })
        }
        _ => return Err(CliError::usage("--swipe may be given once")),
    };
    if swipe.is_some() && (tap.is_some() || tap_rect.is_some()) {
        return Err(CliError::usage(
            "do --capture --record takes --swipe or --tap/--tap-rect, not both",
        ));
    }
    let lock =
        crate::commands::record_flag_begin(global, flags, &instance, "do --capture --record")?;
    let plan = record_plan_click(
        &lock,
        &PlanClickRequest {
            tap,
            tap_rect,
            swipe,
        },
    )?;
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
    // Workflow #336 PKG-S2 (option 丙): the carrier package by its `package_id`.
    let carrier = json!({
        "package_label": prepared.package_label
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

// #411 A19 (exec key 93): a timed-out arrival answers `lab_arrival_timeout` (exit 3); `arrival`
// is the answer's last member.
#[cfg(test)]
#[path = "../tests/gate_0122_a19.rs"]
mod gate_0122_a19;

// #411 Lab L3 (exec key 97): `do --capture --record` and `ensure` press a long press as a long
// tap.
#[cfg(test)]
#[path = "../tests/gate_lab_l3.rs"]
mod gate_lab_l3;
