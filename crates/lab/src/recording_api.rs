// SPDX-License-Identifier: AGPL-3.0-only

//! Public entry points of the Lab recording core (Workflow #336 L3). Every state-writing
//! call takes the per-instance `RecordingLock`, so the lock is taken before any write;
//! `record_status` only reads.

use crate::recording::marks::match_metric;
use crate::recording::model::{
    ApplicationPlan, AttachFrameOutcome, AttachFrameRequest, ClickEffect, ClickPlan, ClickView,
    CommitApplicationOutcome, CommitApplicationRequest, CommitClickOutcome, CommitClickRequest,
    FrameView, LAB_RECORD_MARK_SCHEMA, LabRecordingStart, LabStatus, LabStatusView, MarkOutcome,
    MarkRequest, MarkStatusView, MarkView, PlanApplicationRequest, PlanClickRequest,
    RecordStartOptions, RecordedFrame, RecordingDefaults, RecordingStep, StepClick, StepStateView,
    StepTransition, StepView, TransitionSpec, TransitionView,
};
use crate::recording::steps::{
    ArrivalMode, FrameMeta, application_action, application_with_click, apply_marks,
    apply_step_action, apply_transition, arrive, check_device_arrival, commit_application,
    commit_click, default_recording_defaults, effective_indices, new_recording, open_session,
    open_step, plan_application, plan_click, save, to_json,
};
use crate::recording::store::{
    LabFile, blocked, create_recording, invalid, lab_dir, load_lab, now_unix_ms, page_name_valid,
    read_old_record, recording_path, save_recording, state_io, task_id_valid_for_lab, with_details,
};
use crate::recording::{RecordingLock, frames::decode_frame};
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use serde_json::json;
use std::path::Path;

/// Defaults of a new recording from `record start` options (validated before any write).
pub fn record_start_defaults(options: &RecordStartOptions) -> LabResult<RecordingDefaults> {
    let mut defaults = default_recording_defaults();
    if let Some(metric) = &options.match_metric {
        match_metric(metric)?;
        defaults.match_metric = metric.clone();
    }
    if let Some(threshold) = options.template_threshold {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(invalid(
                "validation_failed",
                "--template-threshold must be in 0..=1",
            ));
        }
        defaults.template_threshold = threshold;
    }
    Ok(defaults)
}

/// Creates the Lab recording of the session the `record start` command just wrote. A task
/// id that cannot name a package task, or a reused record id, leaves the Lab part
/// unavailable while the session itself runs on.
pub fn record_start(
    lock: &RecordingLock,
    options: &RecordStartOptions,
) -> LabResult<LabRecordingStart> {
    let defaults = record_start_defaults(options)?;
    let state_dir = lock.state_dir();
    let old = read_old_record(state_dir, lock.instance())?
        .ok_or_else(|| state_io("the record start file is missing after it was written"))?;
    let path = recording_path(state_dir, &old.record_id)
        .display()
        .to_string();
    let unavailable = |reason: &str| LabRecordingStart {
        status: "unavailable".to_string(),
        reason: Some(reason.to_string()),
        path: path.clone(),
        record_id: old.record_id.clone(),
        defaults: defaults.clone(),
    };
    if !task_id_valid_for_lab(&old.task_id) {
        return Ok(unavailable("task_id_invalid_for_lab_package"));
    }
    if lab_dir(state_dir, &old.record_id).exists() {
        return Ok(unavailable("record_id_reused"));
    }
    let recording = new_recording(
        &old,
        defaults.clone(),
        options.game.clone(),
        options.server.clone(),
        options.locale.clone(),
    );
    create_recording(state_dir, &recording)?;
    Ok(LabRecordingStart {
        status: "active".to_string(),
        reason: None,
        path,
        record_id: old.record_id,
        defaults,
    })
}

/// The `lab` field of `record status`: none without a Lab recording, unavailable with a
/// reason, or the recording view.
pub fn record_status(state_dir: &Path, instance: &str) -> LabResult<Option<LabStatus>> {
    let Some(old) = read_old_record(state_dir, instance)? else {
        return Ok(None);
    };
    let unavailable = |reason: &str| {
        Some(LabStatus::Unavailable {
            status: "unavailable".to_string(),
            reason: reason.to_string(),
        })
    };
    match load_lab(state_dir, &old)? {
        LabFile::Missing if task_id_valid_for_lab(&old.task_id) => Ok(None),
        LabFile::Missing => Ok(unavailable("task_id_invalid_for_lab_package")),
        LabFile::Unavailable(reason) => Ok(unavailable(reason)),
        LabFile::Ready(recording) => {
            let numbers = effective_indices(&recording);
            let steps = recording
                .steps
                .iter()
                .map(|step| step_view(step, &numbers))
                .collect();
            Ok(Some(LabStatus::Recording(Box::new(LabStatusView {
                record_id: recording.record_id.clone(),
                status: recording.status.clone(),
                coordinate_space: recording.coordinate_space,
                defaults: recording.defaults.clone(),
                open_step: open_step(&recording),
                steps,
                artifact: recording.artifact.clone(),
            }))))
        }
    }
}

/// Before `record stop` changes anything: a Lab recording with steps is generated by the
/// package generator, which this build does not contain; both states stay active.
pub fn record_stop_precheck(lock: &RecordingLock) -> LabResult<()> {
    let state_dir = lock.state_dir();
    let Some(old) = read_old_record(state_dir, lock.instance())? else {
        return Ok(());
    };
    if let LabFile::Ready(recording) = load_lab(state_dir, &old)?
        && !recording.steps.is_empty()
    {
        return Err(LabError::not_implemented(
            "record_stop_generation_not_implemented",
            format!(
                "the Lab recording {} has {} step(s); generating its package on record stop \
                 is not part of this build; the recording and the session stay active",
                recording.record_id,
                recording.steps.len()
            ),
        )
        .with_details(json!({
            "record_id": recording.record_id,
            "steps": recording.steps.len(),
            "lab_status": recording.status
        })));
    }
    Ok(())
}

/// After `record stop` stopped the session: an active Lab recording without steps is
/// stopped too, so both states are closed.
pub fn record_stop_close(lock: &RecordingLock) -> LabResult<Option<String>> {
    let state_dir = lock.state_dir();
    let Some(old) = read_old_record(state_dir, lock.instance())? else {
        return Ok(None);
    };
    if let LabFile::Ready(mut recording) = load_lab(state_dir, &old)?
        && recording.steps.is_empty()
        && recording.status == "active"
    {
        recording.status = "stopped".to_string();
        recording.updated_at_unix_ms = now_unix_ms();
        let path = save_recording(state_dir, &recording)?;
        return Ok(Some(path.display().to_string()));
    }
    Ok(None)
}

fn validate_mark_request(request: &MarkRequest) -> LabResult<()> {
    if request.schema_version != LAB_RECORD_MARK_SCHEMA {
        return Err(invalid(
            "validation_failed",
            format!("record mark requests use schema_version {LAB_RECORD_MARK_SCHEMA}"),
        ));
    }
    if let Some(page) = &request.page
        && !page_name_valid(page)
    {
        return Err(invalid(
            "validation_failed",
            format!("--page '{page}' must match ^[a-z0-9][a-z0-9_]{{0,47}}$"),
        ));
    }
    let marks_or_click = request.frame.is_some()
        || !request.samples.is_empty()
        || request.page.is_some()
        || !request.add.is_empty()
        || !request.reuse.is_empty()
        || !request.remove.is_empty()
        || request.click.is_some()
        || request.click_guard.is_some()
        || request.retry.is_some()
        || request.replace_click
        || request.application.is_some();
    if request.step_action.is_some() {
        if marks_or_click
            || request.transition.is_some()
            || request.replace_transition
            || request.step.is_some()
        {
            return Err(invalid(
                "validation_failed",
                "step operations cannot be combined with marks, clicks, application \
                 operations, samples, frames, pages or transitions",
            ));
        }
        return Ok(());
    }
    if let Some(transition) = &request.transition {
        if request.click.is_some()
            || request.click_guard.is_some()
            || request.retry.is_some()
            || request.replace_click
            || request.application.is_some()
        {
            return Err(invalid(
                "record_transition_has_click",
                "a transition only recognizes; it cannot carry a click or an application \
                 operation",
            ));
        }
        if request.frame.is_some()
            || !request.samples.is_empty()
            || request.page.is_some()
            || !request.add.is_empty()
            || !request.reuse.is_empty()
            || !request.remove.is_empty()
        {
            return Err(invalid(
                "validation_failed",
                "a transition takes its frame, samples and marks inside the transition object",
            ));
        }
        if request.step.is_none() {
            return Err(invalid(
                "validation_failed",
                "a transition needs --step <k>: the step whose click it follows",
            ));
        }
        if let TransitionSpec::Clear = transition
            && request.replace_transition
        {
            return Err(invalid(
                "validation_failed",
                "--replace-transition does not apply to --transition none",
            ));
        }
    }
    if let Some(application) = &request.application {
        application_action(&application.action)?;
        if request.click.is_some() || request.click_guard.is_some() || request.retry.is_some() {
            return Err(application_with_click(request.step));
        }
    }
    Ok(())
}

/// `record mark`: marks, clicks, samples and pages; transitions; or one step operation.
/// Everything is checked and self-tested first and written once; `dry_run` writes nothing.
pub fn record_mark(
    lock: &RecordingLock,
    request: &MarkRequest,
    dry_run: bool,
) -> LabResult<MarkOutcome> {
    validate_mark_request(request)?;
    let mut session = open_session(lock)?;
    let mut outcome = MarkOutcome {
        status: if dry_run {
            "marks_validated"
        } else {
            "marks_recorded"
        }
        .to_string(),
        record_id: session.recording.record_id.clone(),
        step: None,
        step_opened: false,
        frame: None,
        samples: Vec::new(),
        marks: Vec::new(),
        reused: Vec::new(),
        removed: Vec::new(),
        click: None,
        application: None,
        transition: None,
        step_state: None,
        closed_step: None,
        dry_run,
    };
    let step = if let Some(action) = &request.step_action {
        let (status, step) = apply_step_action(&mut session, action)?;
        outcome.status = status.to_string();
        step
    } else if let Some(spec) = &request.transition {
        let applied = apply_transition(&mut session, request, spec)?;
        outcome.marks = applied.marks;
        outcome.reused = applied.reused;
        applied.step
    } else {
        let applied = apply_marks(&mut session, request)?;
        outcome.step_opened = applied.opened;
        outcome.closed_step = applied.closed_step;
        outcome.frame = applied.frame;
        outcome.samples = applied.samples;
        outcome.marks = applied.added;
        outcome.reused = applied.reused;
        outcome.removed = applied.removed;
        applied.step
    };
    outcome.step = Some(step);
    if let Some(recorded) = session
        .recording
        .steps
        .iter()
        .find(|item| item.index == step)
    {
        outcome.click = recorded.click.clone();
        outcome.application = recorded.application.clone();
        outcome.transition = recorded.transition.as_ref().map(transition_view);
        outcome.step_state = Some(step_state(recorded));
    }
    if !dry_run {
        save(&mut session)?;
    }
    Ok(outcome)
}

/// The record instance and the command instance must be the same (`record_instance_mismatch`).
pub fn record_instance_check(record_instance: &str, command_instance: &str) -> LabResult<()> {
    if record_instance != command_instance {
        return Err(with_details(
            blocked(
                "record_instance_mismatch",
                format!(
                    "the recording belongs to instance {record_instance} but the command \
                     targets {command_instance}; pass the same --instance to both"
                ),
            ),
            json!({"record_instance": record_instance, "command_instance": command_instance}),
        ));
    }
    Ok(())
}

/// Before a `--record` capture: an active recording whose open step accepts a device frame.
pub fn record_frame_preflight(lock: &RecordingLock) -> LabResult<()> {
    let session = open_session(lock)?;
    check_device_arrival(&session.recording)
}

/// Records a frame from `capture --record` or `observe --capture --record`.
pub fn record_attach_frame(
    lock: &RecordingLock,
    request: AttachFrameRequest,
) -> LabResult<AttachFrameOutcome> {
    let mut session = open_session(lock)?;
    let label = format!("{} frame", request.source);
    let loaded = decode_frame(request.png, &label)?;
    let meta = FrameMeta {
        source: request.source,
        runtime_artifact: request.runtime_artifact,
        capture_backend: request.capture_backend,
        freshness: request.freshness,
    };
    let arrival = arrive(&mut session, loaded, ArrivalMode::Device, &meta, &label)?;
    save(&mut session)?;
    Ok(AttachFrameOutcome {
        status: "frame_recorded".to_string(),
        record_id: session.recording.record_id.clone(),
        step: arrival.step,
        step_opened: arrival.opened,
        frame: arrival.frame,
        closed_step: arrival.closed_step,
    })
}

/// Before `do --capture --record` presses anything: the step, rectangle and point.
pub fn record_plan_click(lock: &RecordingLock, request: &PlanClickRequest) -> LabResult<ClickPlan> {
    let session = open_session(lock)?;
    plan_click(&session.recording, request)
}

/// Records the Runtime outcome of a planned click. A Performed click closes its step; one
/// that also reports a failure is kept for review (exit 4); an indeterminate one stays open
/// (exit 4). A failure to record after the input says so (`record_append_failed_after_input`).
pub fn record_commit_click(
    lock: &RecordingLock,
    plan: &ClickPlan,
    request: &CommitClickRequest,
) -> LabResult<CommitClickOutcome> {
    let recorded = (|| -> LabResult<CommitClickOutcome> {
        let mut session = open_session(lock)?;
        let (click, performed) = commit_click(&mut session.recording, plan, request)?;
        save(&mut session)?;
        Ok(CommitClickOutcome {
            status: if performed {
                "click_recorded"
            } else {
                "click_indeterminate"
            }
            .to_string(),
            record_id: session.recording.record_id.clone(),
            step: plan.step,
            step_closed: performed,
            click,
        })
    })();
    let effect = match request.effect {
        ClickEffect::Performed => "performed",
        ClickEffect::Indeterminate => "indeterminate",
    };
    let outcome = recorded.map_err(|error| {
        let cause = json!({
            "code": error.code,
            "message": error.message,
            "details": error.details
        });
        LabError::new(
            LabErrorClass::SafetyBlocked,
            "record_append_failed_after_input",
            format!(
                "the click was sent to the device ({effect}) but recording it failed: {}: {}",
                error.code, error.message
            ),
            &["session_record"],
        )
        .with_details(json!({"plan": to_json(plan), "effect": effect, "cause": cause}))
    })?;
    if request.effect == ClickEffect::Indeterminate {
        return Err(LabError::new(
            LabErrorClass::DeviceInstance,
            "record_click_indeterminate",
            format!(
                "the click of step {} has an indeterminate outcome; it is recorded and the step \
                 stays open; check the screen, then `record mark --reopen-step {}` to retry or \
                 `record mark --close-step` to accept it",
                plan.step, plan.step
            ),
            &["device"],
        )
        .with_details(json!({"record": to_json(&outcome)})));
    }
    if request.has_failure {
        return Err(LabError::new(
            LabErrorClass::DeviceInstance,
            "record_click_performed_with_failure",
            format!(
                "the click of step {} was performed but the operation reported a failure; it is \
                 recorded, the step is closed and marked needs_review",
                plan.step
            ),
            &["device"],
        )
        .with_details(json!({"record": to_json(&outcome)})));
    }
    Ok(outcome)
}

/// Before `session app <verb> --record` sends anything: the step the application operation
/// lands on (Workflow #336 R24). Every refusal means nothing was sent.
pub fn record_plan_application(
    lock: &RecordingLock,
    request: &PlanApplicationRequest,
) -> LabResult<ApplicationPlan> {
    let session = open_session(lock)?;
    plan_application(&session.recording, request)
}

/// Records the Runtime result of a planned application operation. Performed closes the step
/// (`application_recorded`); Indeterminate records one more attempt, keeps the step open and
/// returns `record_application_indeterminate` (exit 4). A failure to record after the
/// operation was sent is `record_append_failed_after_input`. Only these two errors mean the
/// application operation may have run.
pub fn record_commit_application(
    lock: &RecordingLock,
    plan: &ApplicationPlan,
    request: &CommitApplicationRequest,
) -> LabResult<CommitApplicationOutcome> {
    let recorded = (|| -> LabResult<CommitApplicationOutcome> {
        let mut session = open_session(lock)?;
        let (application, opened, closed) =
            commit_application(&mut session.recording, plan, request)?;
        save(&mut session)?;
        Ok(CommitApplicationOutcome {
            status: if closed {
                "application_recorded"
            } else {
                "application_indeterminate"
            }
            .to_string(),
            record_id: session.recording.record_id.clone(),
            step: plan.step,
            step_opened: opened,
            step_closed: closed,
            application,
        })
    })();
    let effect = match request {
        CommitApplicationRequest::Performed(_) => "performed",
        CommitApplicationRequest::Indeterminate(_) => "indeterminate",
    };
    let outcome =
        recorded.map_err(|error| record_application_append_failed(plan, effect, error))?;
    if let CommitApplicationRequest::Indeterminate(attempt) = request {
        return Err(LabError::new(
            LabErrorClass::DeviceInstance,
            "record_application_indeterminate",
            format!(
                "the application operation {} of step {} has no completed receipt (receipt {}, \
                 Runtime code {}); it may have run. The attempt is recorded and the step stays \
                 open: check the instance, then run `session app {} --record` again (launch, \
                 restart and stop can be repeated) or accept it with `record mark --close-step`",
                plan.action,
                plan.step,
                attempt.receipt_state,
                attempt.runtime_code.as_deref().unwrap_or("none"),
                plan.cli_verb
            ),
            &["device"],
        )
        .with_details(json!({
            "runtime_code": attempt.runtime_code,
            "receipt_state": attempt.receipt_state,
            "request_id": attempt.request_id,
            "record": to_json(&outcome)
        })));
    }
    Ok(outcome)
}

/// The application operation was sent (`effect`: `performed` or `indeterminate`) but could
/// not be recorded: `record_append_failed_after_input` (exit 3) with the plan and the cause.
pub fn record_application_append_failed(
    plan: &ApplicationPlan,
    effect: &str,
    cause: LabError,
) -> LabError {
    let details = json!({
        "code": cause.code,
        "message": cause.message,
        "details": cause.details
    });
    LabError::new(
        LabErrorClass::SafetyBlocked,
        "record_append_failed_after_input",
        format!(
            "the application operation was sent ({effect}) but recording it failed: {}: {}",
            cause.code, cause.message
        ),
        &["session_record"],
    )
    .with_details(json!({"plan": to_json(plan), "effect": effect, "cause": details}))
}

fn frame_view(frame: &RecordedFrame) -> FrameView {
    FrameView {
        frame_id: frame.frame_id.clone(),
        role: frame.role.clone(),
        sha256: frame.sha256.clone(),
        w: frame.width,
        h: frame.height,
        superseded: frame.superseded,
    }
}

fn transition_view(transition: &StepTransition) -> TransitionView {
    match transition {
        StepTransition::Page {
            frames,
            marks,
            timeout_ms,
            source,
            ..
        } => TransitionView {
            kind: "page".to_string(),
            frames: Some(frames.iter().map(frame_view).collect()),
            marks: Some(marks.clone()),
            timeout_ms: *timeout_ms,
            source: Some(source.clone()),
            min_ms: None,
            max_ms: None,
        },
        StepTransition::Window { min_ms, max_ms } => TransitionView {
            kind: "window".to_string(),
            frames: None,
            marks: None,
            timeout_ms: None,
            source: None,
            min_ms: Some(*min_ms),
            max_ms: Some(*max_ms),
        },
    }
}

/// `declared` (no outcome yet), `executed` (Performed) or `indeterminate`.
fn click_outcome(click: &StepClick) -> &'static str {
    match &click.execution {
        None => "declared",
        Some(execution) if execution.effect == "performed" => "executed",
        Some(_) => "indeterminate",
    }
}

/// `none`, `click_<declared|executed|indeterminate>` or `application_<declared|executed>`.
fn effect_state(step: &RecordingStep) -> String {
    match (&step.click, &step.application) {
        (Some(click), _) => format!("click_{}", click_outcome(click)),
        (None, Some(application)) if application.executed.is_some() => {
            "application_executed".to_string()
        }
        (None, Some(_)) => "application_declared".to_string(),
        (None, None) => "none".to_string(),
    }
}

fn step_state(step: &RecordingStep) -> StepStateView {
    StepStateView {
        marks: step.marks.len() + step.reused.len(),
        frames: step.frames.iter().filter(|frame| !frame.superseded).count(),
        effect: effect_state(step),
        transition: step
            .transition
            .as_ref()
            .map(|transition| transition.kind().to_string()),
        closed: step.closed,
    }
}

fn step_view(step: &RecordingStep, effective: &[u32]) -> StepView {
    let artifact_step = effective
        .iter()
        .position(|index| *index == step.index)
        .and_then(|position| u32::try_from(position + 1).ok());
    StepView {
        index: step.index,
        artifact_step,
        entry: if step.is_application_entry() {
            "any"
        } else {
            "page"
        }
        .to_string(),
        page: step.page.clone(),
        dropped: step.dropped,
        converted_to_transition: step.converted_to_transition,
        frames: step.frames.iter().map(frame_view).collect(),
        marks: step
            .marks
            .iter()
            .map(|mark| MarkView {
                id: mark.id.clone(),
                family: mark.family,
                self_test: MarkStatusView {
                    status: mark.self_test.status,
                },
                margin: mark.self_test.margin.clone(),
            })
            .collect(),
        reused: step.reused.clone(),
        click: step.click.as_ref().map(|click| ClickView {
            rect: click.rect,
            source: click.source.clone(),
            executed: click.executed(),
            outcome: click_outcome(click).to_string(),
            attempts: click.attempts.len(),
            needs_review: click.needs_review,
        }),
        application: step.application.clone(),
        transition: step.transition.as_ref().map(transition_view),
        closed: step.closed,
        closed_by: step.closed_by.clone(),
    }
}
