// SPDX-License-Identifier: AGPL-3.0-only

//! Public entry points of the Lab recording core (Workflow #336 L3). Every state-writing
//! call takes the per-instance `RecordingLock`, so the lock is taken before any write;
//! `record_status` only reads.

use crate::LabOcr;
use crate::codes::{LabCode, ReadyCheck};
use crate::recording::archive::{ARCHIVED, latest_manifest, read_recording};
use crate::recording::boxes::{expanded_add, validate_boxes, with_press_click};
use crate::recording::container::{
    container_kind, encode, extension, file_name, preflight_lab_dir, replace_in_place,
    write_lab_dir,
};
use crate::recording::crosscheck::{
    CrossCheck, EndingChecks, FirstDecision, FrameCache, SelectChecks, admit, cross_check,
    ending_checks, evaluator_bundle, first_decision, round_trip, select_checks,
};
use crate::recording::ending::{remove_ending, validate_ending_request};
use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, Plan, StopSettings, Survey,
    application_steps_summary, optional_steps_summary, plan as generate_plan, render, survey,
    transitions_summary, validate_timeout_option,
};
use crate::recording::inserted::validate_optional_after_request;
use crate::recording::marks::{declares_mark, match_metric};
use crate::recording::model::{
    AfterCapture, AfterCaptureRecord, AfterFrame, ApplicationPlan, AttachFrameOutcome,
    AttachFrameRequest, BeforeCapture, BeforeFrame, CapturedFrame, ClickEffect, ClickGesture,
    ClickPlan, ClickView, CommitApplicationOutcome, CommitApplicationRequest, CommitClickOutcome,
    CommitClickRequest, FrameView, LAB_RECORD_MARK_SCHEMA, LAB_RECORDING_MAX_NEXT_FRAME_MS,
    LAB_RECORDING_MAX_SETTLE_MS, LabRecording, LabRecordingStart, LabStatus, LabStatusView,
    MarkOutcome, MarkRequest, MarkStatusView, MarkView, NextFrameOutcome, OpaqueJson,
    PlanApplicationRequest, PlanClickRequest, RecordPressOutcome, RecordPressRequest,
    RecordStartOptions, RecordStepOutcome, RecordStepRequest, RecordStopOptions, RecordStopOutcome,
    RecordedFrame, RecordedMark, RecordingArtifact, RecordingDefaults, RecordingStep,
    SelfTestStatus, StepClick, StepOptionalView, StepStateView, StepTransition, StepView,
    TransitionSpec, TransitionView,
};
use crate::recording::ready::{
    RecordReadyOutcome, check_item, generator_warnings, lack_items, packaged, point_press_warnings,
    ready_outcome,
};
use crate::recording::select::{not_evaluated_id, validate_select_request};
use crate::recording::steps::{
    Arrival, ArrivalMode, FrameMeta, Session, application_action, application_with_click,
    apply_ending, apply_marks, apply_optional_after, apply_step_action, apply_transition, arrive,
    arrive_after_press, check_device_arrival, commit_application, commit_click,
    default_recording_defaults, effective_indices, has_marks, lab_unavailable, live_primary,
    name_after_frame, new_recording, not_active, note_after_capture, open_session,
    open_session_for_marks, open_step, path_order, plan_application, plan_click, plan_press,
    remove_inserted, save, step_ref, stopped_change, to_json,
};
use crate::recording::store::{
    LabFile, OldRecordView, blocked, create_recording, invalid, lab_dir, load_lab, now_unix_ms,
    page_name_valid, read_old_record, read_verified, recording_path, save_recording, state_io,
    task_id_valid_for_lab, with_details,
};
use crate::recording::{
    RecordingLock,
    frames::{LoadedFrame, check_frame_size, decode_frame, read_stored_frame},
};
use actingcommand_contract::{
    InputAction, InputFrameReference, LabError, LabErrorClass, LabResult,
};
use actingcommand_pack_containment::ContentContainer;
use actingcommand_recognition::color_digest::{
    ColorDigest, FRAME_CHANGE_MAX_MILLI, frame_change_digest, frame_change_milli,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
            // #463 R4: the steps in the order they run, a step inserted after the stop in its
            // place.
            let steps = path_order(&recording)
                .into_iter()
                .map(|step| step_view(step, &numbers))
                .collect();
            // Workflow #449 REC-ARCH: an archived recording's session is ended; its step table,
            // which archiving never rewrites, still says active.
            let status = if old.status == ARCHIVED {
                ARCHIVED.to_string()
            } else {
                recording.status.clone()
            };
            Ok(Some(LabStatus::Recording(Box::new(LabStatusView {
                record_id: recording.record_id.clone(),
                status,
                coordinate_space: recording.coordinate_space,
                defaults: recording.defaults.clone(),
                open_step: open_step(&recording),
                steps,
                artifact: recording.artifact.clone(),
                amended_at_unix_ms: recording.amended_at_unix_ms,
                endings: recording.endings.clone(),
            }))))
        }
    }
}

/// `record stop` before the session file is stopped (Workflow #336 L4, frozen model section
/// 4): a Lab recording with steps generates its `linear_steps` package, runs every self-check
/// and writes the package (section 4.7) and the stopped recording; a stopped recording with a
/// package copies it to `--lab-dir` again (`already_generated`). `dry_run` runs everything and
/// writes nothing. Any refusal leaves the recording and the session active. Without Lab steps
/// the outcome has no `lab` and `record stop` behaves as before. The self-checks evaluate OCR
/// with `ocr` (Workflow #396 LAB-3).
pub fn record_stop(
    lock: &RecordingLock,
    options: &RecordStopOptions,
    ocr: &LabOcr,
) -> LabResult<RecordStopOutcome> {
    for (name, value) in [
        ("--timeout-ms", options.timeout_ms),
        ("--arrival-timeout-ms", options.arrival_timeout_ms),
        (
            "--application-arrival-timeout-ms",
            options.application_arrival_timeout_ms,
        ),
    ] {
        validate_timeout_option(name, value)?;
    }
    let none = RecordStopOutcome {
        lab_status: None,
        lab: None,
    };
    let state_dir = lock.state_dir();
    let Some(old) = read_old_record(state_dir, lock.instance())? else {
        return Ok(none);
    };
    // Workflow #449 REC-ARCH: an archived recording builds no package until `record reopen` makes
    // it active again; nothing is read or written.
    if old.status == ARCHIVED {
        return Err(with_details(
            not_active(format!(
                "the recording {} is archived; run `record reopen --record-id {}` first, then mark \
                 its steps and stop it",
                old.record_id, old.record_id
            )),
            json!({"reason": "recording_archived", "record_id": old.record_id}),
        ));
    }
    let recording = match load_lab(state_dir, &old)? {
        LabFile::Ready(recording) => *recording,
        LabFile::Missing | LabFile::Unavailable(_) => return Ok(none),
    };
    if recording.status == "stopped" {
        return match &recording.artifact {
            // Workflow #390 LAB-2: marked again after its package was generated, the recording
            // generates its package again.
            Some(artifact) if recording.amended_at_unix_ms.is_some() => {
                let settings = regeneration_settings(artifact, options);
                generate_and_write(state_dir, &old, &recording, &settings, options, ocr, true)
            }
            Some(artifact) => stop_already_generated(state_dir, &recording, artifact, options),
            None => Ok(none),
        };
    }
    if recording.status != "active" {
        return Err(not_active(format!(
            "the Lab recording {} is {}, not active",
            recording.record_id, recording.status
        )));
    }
    if recording.steps.is_empty() {
        return Ok(none);
    }
    let settings = stop_settings(&recording, options);
    generate_and_write(state_dir, &old, &recording, &settings, options, ocr, false)
}

/// The generation options of an active recording: each option `record stop` gives, otherwise the
/// recording's game, server and locale, otherwise (game and server) the instance configuration's
/// (`default_*`), and the default arrival timeouts. #463 S2-1: `record ready` reads them too.
fn stop_settings(recording: &LabRecording, options: &RecordStopOptions) -> StopSettings {
    StopSettings {
        game: options
            .game
            .clone()
            .or_else(|| recording.game.clone())
            .or_else(|| options.default_game.clone()),
        server: options
            .server
            .clone()
            .or_else(|| recording.server.clone())
            .or_else(|| options.default_server.clone()),
        locale: options.locale.clone().or_else(|| recording.locale.clone()),
        package_id: options.package_id.clone(),
        requires: options.requires.clone(),
        timeout_ms: options.timeout_ms,
        arrival_timeout_ms: options
            .arrival_timeout_ms
            .unwrap_or(DEFAULT_ARRIVAL_TIMEOUT_MS),
        application_arrival_timeout_ms: options
            .application_arrival_timeout_ms
            .unwrap_or(DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS),
    }
}

/// Workflow #390 LAB-2: the generation options of a stopped recording marked again: each option
/// `record stop` gives, otherwise the value its package was generated with. The task timeout is
/// computed again unless `--timeout-ms` gives it, since the marks may have changed the steps'
/// effects and transitions it is computed from; `--lab-dir` is written only when given.
fn regeneration_settings(
    artifact: &RecordingArtifact,
    options: &RecordStopOptions,
) -> StopSettings {
    StopSettings {
        game: options.game.clone().or_else(|| artifact.game.clone()),
        server: options.server.clone().or_else(|| artifact.server.clone()),
        locale: options.locale.clone().or_else(|| artifact.locale.clone()),
        package_id: options
            .package_id
            .clone()
            .or_else(|| Some(artifact.package_id.clone())),
        requires: options
            .requires
            .clone()
            .or_else(|| artifact.requires.clone()),
        timeout_ms: options.timeout_ms,
        arrival_timeout_ms: options
            .arrival_timeout_ms
            .or(artifact.arrival_timeout_ms)
            .unwrap_or(DEFAULT_ARRIVAL_TIMEOUT_MS),
        application_arrival_timeout_ms: options
            .application_arrival_timeout_ms
            .or(artifact.application_arrival_timeout_ms)
            .unwrap_or(DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS),
    }
}

/// #463 S2-1 (`contracts/lab-recording.md`, "What a recording still lacks: `record ready`"):
/// `record ready [--record-id <id>]`, everything the recording still lacks before `record stop` can
/// make its package, in one read: offline, under no lock, writing nothing. The recording is the one
/// `record status` reads (the instance's, by its session header) or the one `record_id` names
/// (active, archived or stopped). Every pre-check of the generator it fails (`generate::survey`,
/// with the options `record stop` would take from `options`: the instance configuration's game and
/// server); when it fails none, the self-checks `record stop` runs (`self_checks`), on the
/// package it would generate, written nowhere, and the refusal of the first that refuses. A
/// recording that is stopped with its package and unchanged since is `packaged`, and nothing is
/// checked again. A refusal that is no pre-check of the recording, and a failure to read the state
/// (its class `runtime_unavailable`), are the command's own.
pub fn record_ready(
    state_dir: &Path,
    instance: &str,
    record_id: Option<&str>,
    options: &RecordStopOptions,
    ocr: &LabOcr,
) -> LabResult<RecordReadyOutcome> {
    let (recording, session) = ready_recording(state_dir, instance, record_id)?;
    let mut warnings = point_press_warnings(&recording);
    if packaged(&recording) {
        return Ok(ready_outcome(
            &recording,
            session,
            Vec::new(),
            warnings,
            &[],
        ));
    }
    // A stopped recording marked again generates with what its package was generated with, as
    // `record stop` does.
    let settings = match &recording.artifact {
        Some(artifact) if recording.status == "stopped" => regeneration_settings(artifact, options),
        _ => stop_settings(&recording, options),
    };
    let (missing, checks_after) = match survey(&recording, &settings)? {
        Survey::Lacking(lacks) => (lack_items(&recording, &lacks)?, ReadyCheck::ALL.to_vec()),
        Survey::Planned(plan) => match self_checks(&plan, ocr) {
            Ok(checked) => {
                warnings.extend(generator_warnings(&checked.warnings)?);
                (Vec::new(), Vec::new())
            }
            // The state could not be read: no check refused, and the command answers it.
            Err(failure) if failure.error.class == LabErrorClass::RuntimeUnavailable => {
                return Err(failure.error);
            }
            Err(failure) => {
                warnings.extend(generator_warnings(&plan.warnings)?);
                let after = ReadyCheck::ALL
                    .iter()
                    .copied()
                    .skip_while(|check| *check != failure.check)
                    .skip(1)
                    .collect::<Vec<_>>();
                (
                    vec![check_item(&recording, failure.check, &failure.error)],
                    after,
                )
            }
        },
    };
    Ok(ready_outcome(
        &recording,
        session,
        missing,
        warnings,
        &checks_after,
    ))
}

/// #463 S2-1: the recording `record ready` reads and its session's state as `record status` says
/// it. Without `record_id`, or with the one the instance's session header names, the recording
/// the header names (`archived` when the header is, otherwise the step table's status); with
/// another, the step table of that id, which must be the instance's (`record_instance_mismatch`):
/// `archived` when it is active and its directory holds a manifest (a later `record start`
/// replaced its header), otherwise its status. No recording is `record_session_not_active`; a
/// header whose Lab recording is missing or unreadable, or an id no step table has,
/// `record_lab_unavailable` with its reason.
fn ready_recording(
    state_dir: &Path,
    instance: &str,
    record_id: Option<&str>,
) -> LabResult<(LabRecording, String)> {
    let header = read_old_record(state_dir, instance)?;
    if let Some(old) = header
        .as_ref()
        .filter(|old| record_id.is_none_or(|id| id == old.record_id))
    {
        let recording = match load_lab(state_dir, old)? {
            LabFile::Ready(recording) => *recording,
            LabFile::Missing => return Err(lab_unavailable("lab_recording_missing")),
            LabFile::Unavailable(reason) => return Err(lab_unavailable(reason)),
        };
        let session = if old.status == ARCHIVED {
            ARCHIVED.to_string()
        } else {
            recording.status.clone()
        };
        return Ok((recording, session));
    }
    let Some(record_id) = record_id else {
        return Err(with_details(
            not_active(format!(
                "instance {instance} has no recording; name one with --record-id"
            )),
            json!({"instance": instance}),
        ));
    };
    let Some(recording) = read_recording(state_dir, record_id)? else {
        return Err(lab_unavailable("lab_recording_missing")
            .with_details(json!({"reason": "lab_recording_missing", "record_id": record_id})));
    };
    record_instance_check(&recording.instance, instance)?;
    let archived =
        recording.status == "active" && latest_manifest(&lab_dir(state_dir, record_id))?.is_some();
    let session = if archived {
        ARCHIVED.to_string()
    } else {
        recording.status.clone()
    };
    Ok((recording, session))
}

/// Generates, checks and writes the package of `recording` with `settings` (section 4); with
/// `regenerate` (Workflow #390 LAB-2) the recording is a stopped one marked again, whose new
/// package replaces the one of its name and whose `lab.status` is `regenerated`.
fn generate_and_write(
    state_dir: &Path,
    old: &OldRecordView,
    recording: &LabRecording,
    settings: &StopSettings,
    options: &RecordStopOptions,
    ocr: &LabOcr,
    regenerate: bool,
) -> LabResult<RecordStopOutcome> {
    let plan = generate_plan(recording, settings)?;
    let Checked {
        entries,
        kind,
        bytes,
        checks,
        selects,
        endings,
        warnings,
        final_decision,
    } = self_checks(&plan, ocr).map_err(|failure| failure.error)?;

    let ext = extension(kind);
    // Workflow #336 PKG-S5 (M-PKG v1.1 §3.5, D9): the package is `<package_id>.<ext>`, and a
    // file of that name is replaced in place.
    let file_name = file_name(&plan.package_id, kind);
    let copy_path = lab_dir(state_dir, &recording.record_id)
        .join("out")
        .join(&file_name);
    let lab_dir_plan = options
        .lab_dir
        .as_deref()
        .map(|dir| preflight_lab_dir(Path::new(dir), &file_name))
        .transpose()?;
    let mut written = Vec::new();
    // #464 P4: the outputs an earlier stop wrote, set aside and kept.
    let mut kept = Vec::new();
    let lab_dir_status = if options.dry_run {
        None
    } else {
        if let Some(parent) = copy_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                state_io(format!("failed to create {}: {error}", parent.display()))
            })?;
        }
        replace_in_place(&copy_path, &bytes, &mut written, &mut kept)?;
        match &lab_dir_plan {
            Some(dir_plan) => Some(written_status(write_lab_dir(
                dir_plan,
                &bytes,
                &mut written,
                &mut kept,
            )?)),
            None => None,
        }
    };
    let package_path = lab_dir_plan.as_ref().map_or_else(
        || absolute_display(&copy_path),
        |dir_plan| absolute_display(&dir_plan.target),
    );
    if !options.dry_run {
        let mut stopped = recording.clone();
        stopped.status = "stopped".to_string();
        stopped.updated_at_unix_ms = now_unix_ms();
        stopped.amended_at_unix_ms = None;
        stopped.artifact = Some(RecordingArtifact {
            container: ext.to_string(),
            path: absolute_display(&copy_path),
            lab_dir_path: lab_dir_plan
                .as_ref()
                .map(|dir_plan| absolute_display(&dir_plan.target)),
            byte_count: bytes.len() as u64,
            package_id: plan.package_id.clone(),
            requires: plan.requires.clone(),
            game: Some(plan.game.clone()),
            server: Some(plan.server.clone()),
            locale: Some(plan.locale.clone()),
            timeout_ms: Some(plan.timeout_ms),
            arrival_timeout_ms: Some(plan.arrival_timeout_ms),
            application_arrival_timeout_ms: Some(plan.application_arrival_timeout_ms),
            generated_at_unix_ms: now_unix_ms(),
        });
        save_recording(state_dir, &stopped).map_err(|error| {
            let mut details = error.details.clone().unwrap_or_else(|| json!({}));
            if let Some(object) = details.as_object_mut() {
                object.insert("written_files".to_string(), json!(written));
            }
            error.with_details(details)
        })?;
    }

    let status = if options.dry_run {
        "validated"
    } else if regenerate {
        "regenerated"
    } else {
        "generated"
    };
    let mut lab = Map::new();
    lab.insert("status".to_string(), json!(status));
    lab.insert("dry_run".to_string(), json!(options.dry_run));
    if old.status != "active" && !regenerate {
        lab.insert("session_already_stopped".to_string(), json!(true));
    }
    lab.insert("container".to_string(), json!(ext));
    lab.insert("package_id".to_string(), json!(plan.package_id));
    lab.insert("task_id".to_string(), json!(plan.task_id));
    lab.insert("path".to_string(), json!(absolute_display(&copy_path)));
    lab.insert(
        "lab_dir_path".to_string(),
        json!(
            lab_dir_plan
                .as_ref()
                .map(|dir_plan| absolute_display(&dir_plan.target))
        ),
    );
    if options.dry_run {
        lab.insert(
            "lab_dir_status".to_string(),
            json!(lab_dir_plan.as_ref().map(|dir_plan| if dir_plan.replaces {
                "to_replace"
            } else {
                "to_write"
            })),
        );
        lab.insert("lab_dir_created".to_string(), json!(false));
        lab.insert(
            "lab_dir_to_create".to_string(),
            json!(
                lab_dir_plan
                    .as_ref()
                    .is_some_and(|dir_plan| dir_plan.create_dir)
            ),
        );
        let mut would_write = vec![absolute_display(&copy_path)];
        if let Some(dir_plan) = lab_dir_plan.as_ref() {
            would_write.push(absolute_display(&dir_plan.target));
        }
        lab.insert("would_write".to_string(), json!(would_write));
    } else {
        lab.insert("lab_dir_status".to_string(), json!(lab_dir_status));
        lab.insert(
            "lab_dir_created".to_string(),
            json!(
                lab_dir_plan
                    .as_ref()
                    .is_some_and(|dir_plan| dir_plan.create_dir)
            ),
        );
        lab.insert("written_files".to_string(), json!(written));
        if !kept.is_empty() {
            lab.insert("kept".to_string(), json!(kept));
        }
    }
    lab.insert("byte_count".to_string(), json!(bytes.len()));
    lab.insert(
        "entries".to_string(),
        json!(entries.keys().collect::<Vec<_>>()),
    );
    lab.insert("steps".to_string(), json!(plan.steps.len()));
    lab.insert(
        "click_steps".to_string(),
        json!(
            plan.steps
                .iter()
                .filter(|step| step.step.click.is_some())
                .count()
        ),
    );
    lab.insert(
        "application_steps".to_string(),
        json!(application_steps_summary(&plan)),
    );
    // #463 R2: the endings' pages after the steps' and the transitions'.
    let pages = plan
        .steps
        .iter()
        .flat_map(|step| step.page.iter().chain(step.transition_page.iter()))
        .chain(plan.endings.iter().map(|ending| &ending.page))
        .cloned()
        .collect::<Vec<_>>();
    lab.insert("pages".to_string(), json!(pages));
    lab.insert("transitions".to_string(), json!(transitions_summary(&plan)));
    lab.insert(
        "optional_steps".to_string(),
        json!(optional_steps_summary(&plan, &checks.same_as)?),
    );
    // #463 R1: the select steps with their decisions on every live frame, present only when the
    // package has one.
    if !selects.selects.is_empty() {
        lab.insert("selects".to_string(), json!(selects.selects));
    }
    // #463 R2: the endings with item 9's results, present only when the recording has one.
    if !endings.endings.is_empty() {
        lab.insert("endings".to_string(), json!(endings.endings));
    }
    lab.insert(
        "timeouts".to_string(),
        json!({
            "timeout_ms": plan.timeout_ms,
            "step_timeout_ms": plan.step_timeout_ms,
            "arrival_timeout_ms": plan.arrival_timeout_ms,
            "application_arrival_timeout_ms": plan.application_arrival_timeout_ms
        }),
    );
    lab.insert("warnings".to_string(), json!(warnings));
    lab.insert(
        "arrival_by_time_window".to_string(),
        json!(checks.arrival_by_time_window),
    );
    // #463 R1: a select whose mark-time self-test was not evaluated is listed as `select/<page>`.
    let mut not_evaluated = plan
        .emitted
        .iter()
        .filter(|id| {
            plan.marks
                .get(*id)
                .is_some_and(|mark| mark.self_test.status == SelfTestStatus::NotEvaluated)
        })
        .cloned()
        .collect::<Vec<_>>();
    for (step, select) in plan.select_steps() {
        if select.self_test.status == SelfTestStatus::NotEvaluated {
            not_evaluated.push(not_evaluated_id(step.page_id()?));
        }
    }
    lab.insert("marks_not_evaluated".to_string(), json!(not_evaluated));
    lab.insert("steps_needing_review".to_string(), json!(plan.needs_review));
    lab.insert("cross_check".to_string(), checks.cross_check);
    lab.insert("first_decision".to_string(), final_decision.value);
    lab.insert("entry_overlay".to_string(), checks.entry_overlay);
    insert_binding(
        &mut lab,
        &plan.package_id,
        plan.requires.as_deref(),
        &package_path,
        registry_entry_example(&plan.package_id, &plan.game, &plan.server, &plan.task_id),
    );
    lab.insert(
        "adoption_example".to_string(),
        adoption_example(&recording.record_id, state_dir),
    );
    lab.insert(
        "binding_requires".to_string(),
        json!(binding_requires(&plan)),
    );
    Ok(RecordStopOutcome {
        lab_status: Some(status.to_string()),
        lab: Some(OpaqueJson::from_serializable(&Value::Object(lab))?),
    })
}

/// What `record stop`'s self-checks leave when every one passed: the final package, encoded, and
/// the results its answer prints.
struct Checked {
    entries: BTreeMap<String, Vec<u8>>,
    kind: ContentContainer,
    bytes: Vec<u8>,
    checks: CrossCheck,
    selects: SelectChecks,
    endings: EndingChecks,
    warnings: Vec<Value>,
    final_decision: FirstDecision,
}

/// #463 S2-1: the self-check that refused, by its name, and its refusal.
struct SelfCheckFailure {
    check: ReadyCheck,
    error: LabError,
}

/// #463 S2-1: a refusal of the self-check `check`.
fn failed(check: ReadyCheck) -> impl FnOnce(LabError) -> SelfCheckFailure {
    move |error| SelfCheckFailure { check, error }
}

/// The self-checks of the package `plan` generates (section 4.6), in memory, in the order they
/// run: its stored frames and crops read back, admission and the evaluator, the first decision,
/// the cross-check, the endings, the selects, and the final package encoded, expanded, admitted
/// and decided again. Each stops at its first refusal, and the first refusal ends them: a later
/// check reads what an earlier one gave. Nothing is written. #463 S2-1: `record stop` and `record
/// ready` both run them here; the failure names the check that refused.
fn self_checks(plan: &Plan, ocr: &LabOcr) -> Result<Checked, SelfCheckFailure> {
    let (cache, crops) = load_package_inputs(plan).map_err(failed(ReadyCheck::Frames))?;

    // First pass: admit the package, decide its first step and cross-check its pages.
    let first_entries = render(plan, None, &crops).map_err(failed(ReadyCheck::Admission))?;
    let first_prepared = admit(&first_entries, ocr).map_err(failed(ReadyCheck::Admission))?;
    let bundle = evaluator_bundle(&first_entries, ocr).map_err(failed(ReadyCheck::Admission))?;
    let evaluator = bundle.loaded_bundle().evaluator().ok_or_else(|| {
        failed(ReadyCheck::Admission)(
            LabError::safety_blocked(
                "record_artifact_admission_failed",
                "the admitted package has no recognition evaluator",
                &["session_record"],
            )
            .with_details(json!({"stage": "evaluator"})),
        )
    })?;
    // Workflow #396 REC-4: the evaluator gives the box a followed template matches on a frame.
    let decision = first_decision(&first_prepared, plan, &cache, ocr, evaluator)
        .map_err(failed(ReadyCheck::FirstDecision))?;
    let mut checks =
        cross_check(plan, evaluator, &cache, ocr).map_err(failed(ReadyCheck::CrossCheck))?;
    // #463 R2, self-check item 9, before item 8: every ending's page passes on its own frames and
    // on no frame of a screen the run may show instead (an ending page that passes on a select
    // step's frames before its last press is the ending's refusal, not the select's); the result
    // is part of the cross-check, present only when the recording has an ending.
    let endings =
        ending_checks(plan, evaluator, &cache, ocr).map_err(failed(ReadyCheck::Endings))?;
    if let Some(cross) = checks.cross_check.as_object_mut()
        && !endings.endings.is_empty()
    {
        cross.insert("endings".to_string(), json!(endings.endings));
        if endings.partial {
            cross.insert("status".to_string(), json!("partially_evaluated"));
        }
    }
    // #463 R1, self-check item 8: every select step decides on its own frames as recorded; the
    // result is part of the cross-check, present only when the package has a select step.
    let selects =
        select_checks(&first_prepared, plan, &cache, ocr).map_err(failed(ReadyCheck::Selects))?;
    if let Some(cross) = checks.cross_check.as_object_mut()
        && !selects.selects.is_empty()
    {
        cross.insert("selects".to_string(), json!(selects.selects));
        if !selects.not_evaluated.is_empty() {
            cross.insert(
                "selects_not_evaluated".to_string(),
                json!(selects.not_evaluated),
            );
            cross.insert("status".to_string(), json!("partially_evaluated"));
        }
    }
    let mut warnings = plan.warnings.clone();
    warnings.extend(decision.warning.clone());
    warnings.extend(checks.warnings.clone());
    warnings.extend(endings.warnings.clone());
    let mut provenance = Map::new();
    provenance.insert("warnings".to_string(), json!(warnings));
    provenance.insert(
        "arrival_by_time_window".to_string(),
        json!(checks.arrival_by_time_window),
    );
    provenance.insert("cross_check".to_string(), checks.cross_check.clone());

    // Final package: the same content with the self-check results in its provenance, encoded,
    // expanded again and admitted again.
    let entries =
        render(plan, Some(&provenance), &crops).map_err(failed(ReadyCheck::FinalPackage))?;
    let kind = container_kind(&entries);
    let bytes = encode(&entries, kind).map_err(failed(ReadyCheck::FinalPackage))?;
    round_trip(&bytes, kind, &entries).map_err(failed(ReadyCheck::FinalPackage))?;
    let prepared = admit(&entries, ocr).map_err(failed(ReadyCheck::FinalPackage))?;
    let final_decision = first_decision(&prepared, plan, &cache, ocr, evaluator)
        .map_err(failed(ReadyCheck::FinalPackage))?;
    if final_decision.value != decision.value {
        return Err(failed(ReadyCheck::FinalPackage)(
            LabError::safety_blocked(
                "record_artifact_admission_failed",
                "the first decision of the final package differs from the checked one",
                &["session_record"],
            )
            .with_details(json!({
                "stage": "first_decision",
                "checked": decision.value,
                "final": final_decision.value
            })),
        ));
    }
    Ok(Checked {
        entries,
        kind,
        bytes,
        checks,
        selects,
        endings,
        warnings,
        final_decision,
    })
}

/// Section 4.1 item 7: every live frame and template crop is read back after its sha256
/// check, and every frame has the recording's size.
fn load_package_inputs(plan: &Plan) -> LabResult<(FrameCache, BTreeMap<String, Vec<u8>>)> {
    let mut cache = FrameCache::new();
    for step in &plan.steps {
        for frame in step
            .live_frames()
            .into_iter()
            .chain(step.transition_live_frames())
        {
            if cache.contains_key(&frame.sha256) {
                continue;
            }
            let loaded = read_stored_frame(frame)?;
            check_frame_size(Some(plan.size), &loaded, &frame.frame_id)?;
            cache.insert(frame.sha256.clone(), loaded);
        }
    }
    // #463 R2: and every live frame of an ending.
    for frame in plan.endings.iter().flat_map(|ending| ending.live_frames()) {
        if cache.contains_key(&frame.sha256) {
            continue;
        }
        let loaded = read_stored_frame(frame)?;
        check_frame_size(Some(plan.size), &loaded, &frame.frame_id)?;
        cache.insert(frame.sha256.clone(), loaded);
    }
    let mut crops = BTreeMap::new();
    for (id, _asset) in plan.template_assets() {
        let crop = plan
            .mark(&id)?
            .crop
            .as_ref()
            .ok_or_else(|| LabError::usage(format!("template '{id}' has no crop")))?;
        let bytes = read_verified(Path::new(&crop.path), &crop.sha256)?;
        crops.insert(id, bytes);
    }
    Ok((cache, crops))
}

/// `lab_dir_status` of a written package: `replaced` when a file of its name was there (D9),
/// `written` otherwise.
fn written_status(replaced: bool) -> &'static str {
    if replaced { "replaced" } else { "written" }
}

/// The absolute form of a path for the output and the binding snippets.
fn absolute_display(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Where the package sits in its game's standard-pack registry, the one place another package
/// finds it as a prerequisite package (Workflow #373 NAV-3a, M-373 §4.3; the retired actingd
/// key `prerequisite_packages` is no such place): the game root's `bundle.json` lists it with
/// this `packs[]` entry (`package bundle` writes it and adds `file_count` and `byte_count`), and
/// its directory is the game root plus its `package_id`. The configured root stays a placeholder.
fn registry_entry_example(
    package_id: &str,
    game: &str,
    server: &str,
    entry_task_id: &str,
) -> Value {
    json!({
        "bundle": format!("<standard_packs.root>/{game}/bundle.json"),
        "pack": {
            "package_id": package_id,
            "server": server,
            "entry_task_id": entry_task_id,
            "path": format!("packs/{package_id}")
        },
        "directory": format!("<standard_packs.root>/{game}/{package_id}")
    })
}

/// `binding_example`, `task_run_example`, `prerequisite_entry_example` (its registry entry,
/// [`registry_entry_example`]) and `catalog_on_failure_example` (section 4.8 with a15, a20 and
/// R22). Workflow #336 PKG-S5 (option 丙): each names the package by its path and its
/// `package_id`, never a digest.
fn insert_binding(
    lab: &mut Map<String, Value>,
    package_id: &str,
    requires: Option<&str>,
    package_path: &str,
    prerequisite_entry_example: Value,
) {
    lab.insert("requires".to_string(), json!(requires));
    lab.insert(
        "prerequisite_entry_example".to_string(),
        prerequisite_entry_example,
    );
    lab.insert(
        "binding_example".to_string(),
        json!({
            "procedure_ref": package_id,
            "operation_id": "operation.contained_task",
            "yield_points": [],
            "scheduled_execution": {"mode": "device_registry", "package_path": package_path}
        }),
    );
    lab.insert(
        "task_run_example".to_string(),
        json!(format!(
            "actingctl task-run --state-root <root> --instance <alias> --package \"{package_path}\""
        )),
    );
    lab.insert(
        "catalog_on_failure_example".to_string(),
        json!({
            "action": "pause",
            "retry_limit": 1,
            "retry_backoff_ms": 0,
            "escalation_threshold": 2
        }),
    );
}

/// #464 P4 (`contracts/lab-recording.md`, "Output"; `contracts/standard-pack-authoring.md`, "The
/// decision document"): how the package becomes a member of its game's standard pack: the
/// `standard-pack adopt` line for this recording (its record id and its state directory, what the
/// recording knows; the checkout and the decision file are the caller's) and the skeleton of the
/// decision it takes. Fix round 2 (the owner's rule: a package not yet run through on a real device
/// is registered and stays held): a decision of the class `held` whose `held.reason` is left
/// blank, and nothing else; adopt accepts it once the caller fills in the reason, and refuses a
/// blank one.
fn adoption_example(record_id: &str, state_dir: &Path) -> Value {
    json!({
        "command": format!(
            "actinglab standard-pack adopt --repo <checkout> --record-id {record_id} --state-dir \
             \"{}\" --decide <decision.json>",
            absolute_display(state_dir)
        ),
        "decision": {
            "schema_version": "actingcommand.pack-decision.v1",
            "class": "held",
            "held": {"reason": ""}
        }
    })
}

/// What a binding of the package also needs (section 4.8 with a15, a20 and R24).
fn binding_requires(plan: &Plan) -> Vec<String> {
    let mut items = vec![
        format!(
            "policy.catalog has a task whose procedure_ref is the binding's procedure_ref \
             ({})",
            plan.package_id
        ),
        "catalog_approval_ids match the approval of the catalog".to_string(),
    ];
    let entry = plan.steps.first().is_some_and(|step| step.is_entry());
    if let Some(requires) = &plan.requires {
        items.push(format!(
            "the installed standard pack of this game lists a package with package_id \
             {requires} in its bundle.json (contracts/linear-steps.md, Prerequisite packages)"
        ));
        items.push(
            "the last package of a prerequisite chain is best a page-graph package: it succeeds \
             with 0 steps when the screen is already on its target page"
                .to_string(),
        );
    } else if !entry {
        items.push(
            "This package declares no prerequisite package. When the screen is not on step 1, \
             the Runtime first runs the return-home package the installed standard pack of \
             this game names for this server, then waits for step 1 (R16), so step 1 should be the \
             final screen of that return-home package (the main interface). A package recorded \
             from another screen declares a prerequisite package that brings the screen to \
             step 1 with --requires. Without a return-home package the run fails \
             with contained_task_linear_entry_unmatched."
                .to_string(),
        );
    }
    items.push(
        "The on_failure of the catalog task is best catalog_on_failure_example: a failure of \
         this package's own steps is run once more, and the same problem again suspends the \
         task (R17). The rerun happens immediately (R22): at the first policy evaluation after \
         retry_backoff_ms (0 in the example), without waiting for the next trigger or for \
         cooldown_ms, while the original normal admission's activity window and budget cycle \
         remain valid. Exhausting that budget or ending that window ends immediate retries; \
         the next normal trigger is required even after budgets reset. Other admission gates \
         still apply. Changing on_failure changes the catalog, which needs a new approval."
            .to_string(),
    );
    if plan.has_application_step() {
        items.push(
            "Application steps run only on a physical instance with an assigned application_id; \
             a fixture instance refuses them with application_effect_requires_assigned_application \
             (invalid_request, denied)."
                .to_string(),
        );
        items.push(
            "When adb fails while the application operation runs, a scheduled run of this \
             linear package is only run again and does not suspend the task (R25-2), unless the \
             run was poisoned."
                .to_string(),
        );
    }
    if entry {
        items.push(
            "This package has no entry recognition: every run first performs its application \
             operation (restart stops the game); it cannot declare a prerequisite package and no \
             return-home fallback runs under it."
                .to_string(),
        );
        items.push(
            "Screens a cold start shows only sometimes (a daily sign-in, a notice, an update \
             prompt) are recorded as optional steps between the title and the main interface \
             (record mark --optional); the main interface step itself stays required."
                .to_string(),
        );
        items.push(
            "This package is best configured as the instance startup_package (the second tier \
             of the stall recovery ladder): expand it into \
             <install root>\\packages\\<game>\\<package_id>\\ and configure {package}. As a \
             prerequisite or return-home package it restarts the game whenever the single-frame \
             check of the layer above fails."
                .to_string(),
        );
    }
    if !plan.runs.is_empty() {
        items.push(
            "Optional steps cover only screens that appear after the click of the step before \
             them: a pop-up later than its settle_ms, a pop-up shown more often than the copies \
             recorded, or a pop-up never recorded makes this package fail loudly; it is not \
             handled automatically."
                .to_string(),
        );
    }
    // #463 R2: a package with an ending may end short of its target page.
    if !plan.endings.is_empty() {
        items.push(
            "This package declares endings: a run that ends on one is a success with its key, and \
             its message is left on the screen for the next task's start. Such a package is no \
             prerequisite and no return-home package (contained_task_prerequisite_incompatible, \
             reason=linear_endings), and it needs a Runtime that admits ending pages (package \
             revision 15)."
                .to_string(),
        );
    }
    items
}

/// A stopped recording is not generated again: a generation option given with a value other
/// than the one its package was generated with is `record_stop_option_conflict` (exit 3)
/// instead of being ignored; an equal value is accepted. A value the package did not record
/// (a package generated before the field existed) cannot be equal and conflicts too.
fn refuse_option_conflict(
    recording: &LabRecording,
    artifact: &RecordingArtifact,
    options: &RecordStopOptions,
) -> LabResult<()> {
    let text = |value: &Option<String>| value.as_ref().map(|value| json!(value));
    let number = |value: Option<u64>| value.map(|value| json!(value));
    let fields = [
        (
            "package_id",
            text(&options.package_id),
            Some(json!(artifact.package_id)),
        ),
        ("game", text(&options.game), text(&artifact.game)),
        ("server", text(&options.server), text(&artifact.server)),
        ("locale", text(&options.locale), text(&artifact.locale)),
        (
            "timeout_ms",
            number(options.timeout_ms),
            number(artifact.timeout_ms),
        ),
        (
            "arrival_timeout_ms",
            number(options.arrival_timeout_ms),
            number(artifact.arrival_timeout_ms),
        ),
        (
            "application_arrival_timeout_ms",
            number(options.application_arrival_timeout_ms),
            number(artifact.application_arrival_timeout_ms),
        ),
    ];
    for (field, given, recorded) in fields {
        let Some(given) = given else {
            continue;
        };
        if recorded.as_ref() == Some(&given) {
            continue;
        }
        return Err(LabError::safety_blocked(
            "record_stop_option_conflict",
            format!(
                "the package of recording {} was generated with {field} {}; the given {field} \
                 {given} would need a new package, and a stopped recording is not generated \
                 again; nothing was written",
                recording.record_id,
                recorded
                    .as_ref()
                    .map_or_else(|| "unrecorded".to_string(), Value::to_string)
            ),
            &["session_record"],
        )
        .with_details(json!({"field": field, "recorded": recorded, "given": given})));
    }
    Ok(())
}

/// `record stop` on a stopped recording with a package: with `--lab-dir` the package is copied
/// there again (section 4.7 steps 2 and 4); nothing is generated.
fn stop_already_generated(
    state_dir: &Path,
    recording: &LabRecording,
    artifact: &RecordingArtifact,
    options: &RecordStopOptions,
) -> LabResult<RecordStopOutcome> {
    if let Some(requires) = &options.requires
        && artifact.requires.as_deref() != Some(requires.as_str())
    {
        return Err(LabError::safety_blocked(
            "record_requires_conflict",
            format!(
                "the package of recording {} was generated with requires {}; --requires {requires} \
                 would need a new package, and a stopped recording is not generated again",
                recording.record_id,
                artifact.requires.as_deref().unwrap_or("none")
            ),
            &["session_record"],
        )
        .with_details(json!({
            "requires": requires,
            "generated_requires": artifact.requires
        })));
    }
    refuse_option_conflict(recording, artifact, options)?;
    let bytes = std::fs::read(&artifact.path).map_err(|error| {
        state_io(format!(
            "failed to read the generated package {}: {error}",
            artifact.path
        ))
    })?;
    let file_name = format!("{}.{}", artifact.package_id, artifact.container);
    let lab_dir_plan = options
        .lab_dir
        .as_deref()
        .map(|dir| preflight_lab_dir(Path::new(dir), &file_name))
        .transpose()?;
    let mut written = Vec::new();
    let mut kept = Vec::new();
    let lab_dir_status = match &lab_dir_plan {
        None => None,
        Some(dir_plan) if options.dry_run => Some(if dir_plan.replaces {
            "to_replace"
        } else {
            "to_write"
        }),
        Some(dir_plan) => Some(written_status(write_lab_dir(
            dir_plan,
            &bytes,
            &mut written,
            &mut kept,
        )?)),
    };
    let package_path = lab_dir_plan.as_ref().map_or_else(
        || artifact.path.clone(),
        |dir_plan| absolute_display(&dir_plan.target),
    );
    let mut lab = Map::new();
    lab.insert("status".to_string(), json!("already_generated"));
    lab.insert("dry_run".to_string(), json!(options.dry_run));
    lab.insert("container".to_string(), json!(artifact.container));
    lab.insert("package_id".to_string(), json!(artifact.package_id));
    lab.insert("task_id".to_string(), json!(recording.task_id));
    lab.insert("path".to_string(), json!(artifact.path));
    lab.insert(
        "lab_dir_path".to_string(),
        json!(
            lab_dir_plan
                .as_ref()
                .map(|dir_plan| absolute_display(&dir_plan.target))
        ),
    );
    lab.insert("lab_dir_status".to_string(), json!(lab_dir_status));
    lab.insert(
        "lab_dir_created".to_string(),
        json!(
            !options.dry_run
                && lab_dir_plan
                    .as_ref()
                    .is_some_and(|dir_plan| dir_plan.create_dir && !written.is_empty())
        ),
    );
    lab.insert("written_files".to_string(), json!(written));
    if !kept.is_empty() {
        lab.insert("kept".to_string(), json!(kept));
    }
    lab.insert("byte_count".to_string(), json!(artifact.byte_count));
    lab.insert(
        "generated_at_unix_ms".to_string(),
        json!(artifact.generated_at_unix_ms),
    );
    // A stopped recording's artifact names the game and server it was generated for.
    insert_binding(
        &mut lab,
        &artifact.package_id,
        artifact.requires.as_deref(),
        &package_path,
        registry_entry_example(
            &artifact.package_id,
            artifact.game.as_deref().unwrap_or("<game>"),
            artifact.server.as_deref().unwrap_or("<server>"),
            &recording.task_id,
        ),
    );
    lab.insert(
        "adoption_example".to_string(),
        adoption_example(&recording.record_id, state_dir),
    );
    Ok(RecordStopOutcome {
        lab_status: Some("already_generated".to_string()),
        lab: Some(OpaqueJson::from_serializable(&Value::Object(lab))?),
    })
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
    // #463 R2: an ending, or the key of one to give up, stands alone in its request.
    if request.ending.is_some() || request.remove_ending.is_some() {
        return validate_ending_request(request);
    }
    // #463 R4: so does an optional step inserted after the stop, or one to give up.
    if request.optional_after.is_some() || request.remove_inserted.is_some() {
        return validate_optional_after_request(request);
    }
    // #463 S2-5: boxes are members of a step: never beside a step operation or a transition, one
    // press box beside no click, each recognize box once.
    validate_boxes(request)?;
    if let Some(page) = &request.page
        && !page_name_valid(page)
    {
        return Err(invalid(
            "validation_failed",
            format!("--page '{page}' must match ^[a-z0-9][a-z0-9_]{{0,47}}$"),
        ));
    }
    validate_optional(request)?;
    // Workflow #390 LAB-2: an anchor is named or cleared, not both.
    if request.anchor.is_some() && request.clear_anchor {
        return Err(invalid(
            "validation_failed",
            "give one of --anchor <id> and --no-anchor",
        ));
    }
    let page_rule = !request.forbid.is_empty() || request.anchor.is_some() || request.clear_anchor;
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
        || request.application.is_some()
        || request.select.is_some()
        || request.clear_select
        || page_rule;
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
            || request.select.is_some()
            || request.clear_select
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
        // Workflow #390 LAB-2: a page transition only requires its marks.
        if page_rule {
            return Err(invalid(
                "validation_failed",
                "a page transition forbids no mark and has no anchor; --forbid, --anchor and \
                 --no-anchor apply to a step's page",
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
    // #463 R1: a select is the step's one effect, beside no click, retry, optional step or
    // application operation, and keeps its shape. #463 S2-5: a press box is the step's click for
    // these rules.
    let pressed = with_press_click(request);
    validate_select_request(&pressed)?;
    if let Some(application) = &request.application {
        application_action(&application.action)?;
        if pressed.click.is_some() || request.click_guard.is_some() || request.retry.is_some() {
            return Err(application_with_click(request.step));
        }
    }
    Ok(())
}

/// `optional` / `optional_settle_ms` (Workflow #339): batched with the marks and the effect
/// of the step, never with a transition or a step operation; a settle needs `optional: true`.
fn validate_optional(request: &MarkRequest) -> LabResult<()> {
    if request.optional.is_none() && request.optional_settle_ms.is_none() {
        return Ok(());
    }
    if request.transition.is_some() || request.step_action.is_some() {
        return Err(invalid(
            "validation_failed",
            "--optional, --not-optional and --settle-ms cannot be combined with --transition or \
             a step operation",
        ));
    }
    let Some(settle_ms) = request.optional_settle_ms else {
        return Ok(());
    };
    if request.optional != Some(true) {
        return Err(invalid("validation_failed", "--settle-ms needs --optional"));
    }
    if settle_ms > LAB_RECORDING_MAX_SETTLE_MS {
        return Err(with_details(
            invalid(
                "record_optional_settle_invalid",
                format!(
                    "--settle-ms must be in 0..={LAB_RECORDING_MAX_SETTLE_MS}, got {settle_ms}"
                ),
            ),
            json!({"settle_ms": settle_ms, "max_settle_ms": LAB_RECORDING_MAX_SETTLE_MS}),
        ));
    }
    Ok(())
}

/// `record mark`: marks, clicks, samples and pages; transitions; or one step operation.
/// Everything is checked and self-tested first and written once; `dry_run` writes nothing.
/// OCR marks are self-tested with `ocr` (Workflow #396 LAB-3).
pub fn record_mark(
    lock: &RecordingLock,
    request: &MarkRequest,
    dry_run: bool,
    ocr: &LabOcr,
) -> LabResult<MarkOutcome> {
    validate_mark_request(request)?;
    // Workflow #390 LAB-2: a stopped recording whose package was generated takes marks on its
    // steps again; the next `record stop` generates the package again.
    let mut session = open_session_for_marks(lock)?;
    if session.stopped {
        if request.step_action.is_some() {
            return Err(stopped_change("a step operation"));
        }
        if request.application.is_some() {
            return Err(stopped_change("an application operation"));
        }
    }
    let outcome = mark_session(&mut session, request, dry_run, ocr)?;
    if !dry_run {
        if session.stopped {
            session.recording.amended_at_unix_ms = Some(now_unix_ms());
        }
        save(&mut session)?;
    }
    Ok(outcome)
}

/// `record mark`'s work on an opened session: every check and the self-test, the session changed
/// in memory only; the caller writes it. #463 L1: the one-call form of `do --capture --record`
/// ([`record_plan_step`]) applies its mark request with it, the same code.
fn mark_session(
    session: &mut Session,
    request: &MarkRequest,
    dry_run: bool,
    ocr: &LabOcr,
) -> LabResult<MarkOutcome> {
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
        forbidden: Vec::new(),
        after_stop: session.stopped,
        select: None,
        ending: None,
        removed_ending: None,
        boxes: Vec::new(),
    };
    // #463 R2: an ending of the stopped recording, taken from its frame or given up; it is no
    // step, so the answer names none.
    if let Some(spec) = &request.ending {
        let applied = apply_ending(session, spec, ocr)?;
        if !dry_run {
            outcome.status = "ending_recorded".to_string();
        }
        outcome.frame = Some(applied.frame);
        outcome.samples = applied.samples;
        outcome.marks = applied.added;
        outcome.reused = applied.reused;
        outcome.ending = Some(applied.ending);
        return Ok(outcome);
    }
    if let Some(key) = &request.remove_ending {
        let removed = remove_ending(session, key)?;
        if !dry_run {
            outcome.status = "ending_removed".to_string();
        }
        outcome.removed_ending = Some(removed);
        return Ok(outcome);
    }
    let step = if let Some(action) = &request.step_action {
        let (status, step) = apply_step_action(session, action)?;
        outcome.status = status.to_string();
        step
    } else if let Some(spec) = &request.optional_after {
        // #463 R4: an optional step inserted after a step of the stopped recording, from a frame;
        // its step state names the recorded step it runs after.
        let applied = apply_optional_after(session, spec, ocr)?;
        if !dry_run {
            outcome.status = "optional_step_inserted".to_string();
        }
        outcome.frame = applied.frame;
        outcome.samples = applied.samples;
        outcome.marks = applied.added;
        outcome.reused = applied.reused;
        applied.step
    } else if let Some(index) = request.remove_inserted {
        remove_inserted(session, index)?;
        if !dry_run {
            outcome.status = "optional_step_removed".to_string();
        }
        index
    } else if let Some(spec) = &request.transition {
        let applied = apply_transition(session, request, spec, ocr)?;
        outcome.marks = applied.marks;
        outcome.reused = applied.reused;
        applied.step
    } else {
        let applied = apply_marks(session, request, ocr)?;
        outcome.step_opened = applied.opened;
        outcome.closed_step = applied.closed_step;
        outcome.frame = applied.frame;
        outcome.samples = applied.samples;
        outcome.marks = applied.added;
        outcome.reused = applied.reused;
        outcome.removed = applied.removed;
        outcome.forbidden = applied.forbidden;
        outcome.boxes = applied.boxes;
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
        // #463 R1: the step's select with its self-test.
        outcome.select = recorded.select.clone();
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

/// Workflow #392 DA-5ab (`contracts/display-adapter.md` section 11): `do --capture --record`
/// presses the open step's point, planned in the recording's coordinates, through a Lab
/// operation, which takes its coordinates in its package's base `base` (width, height). A
/// recording whose frames are of another size (taken on a display of another size) would be
/// pressed in the wrong space: it is refused before anything is pressed or written,
/// `record_frame_size_mismatch`, as a frame of another size than the recording is. A recording
/// of the base's size, and one without a frame yet, pass. Fix round 1: `observe --capture
/// --record` makes the same check before it captures (with [`record_observation_base_check`]
/// after it).
pub fn record_press_base_check(lock: &RecordingLock, base: (u32, u32)) -> LabResult<()> {
    let session = open_session(lock)?;
    match session.recording.coordinate_space {
        Some(space) if (space.width, space.height) != base => Err(with_details(
            blocked(
                "record_frame_size_mismatch",
                format!(
                    "the recording is {}x{}, and a Lab operation observes and presses in its \
                     package's base, {}x{}",
                    space.width, space.height, base.0, base.1
                ),
            ),
            json!({
                "frame": "the package's base",
                "expected": space,
                "actual": {"width": base.0, "height": base.1},
            }),
        )),
        _ => Ok(()),
    }
}

/// Workflow #392 DA-5ab fix round 1 (`contracts/display-adapter.md` section 11):
/// `observe --capture --record` attaches the Runtime's frame to the recording as the device gave
/// it, while its answer speaks its package's base `base` (width, height). On a display of another
/// size (`device`, the observation frame's width and height) the recording would take a device
/// frame beside an answer in the base: refused before anything is written,
/// `record_frame_size_mismatch`, as the recorded press is ([`record_press_base_check`]), until the
/// recording on such a display (DA-5cd). A frame of the base's size passes.
pub fn record_observation_base_check(base: (u32, u32), device: (u32, u32)) -> LabResult<()> {
    if device == base {
        return Ok(());
    }
    Err(with_details(
        blocked(
            "record_frame_size_mismatch",
            format!(
                "the Runtime's frame is {}x{}, and a Lab observation speaks its package's base, \
                 {}x{}; the recording takes no frame of another size",
                device.0, device.1, base.0, base.1
            ),
        ),
        json!({
            "frame": "the Runtime observation's frame",
            "expected": {"width": base.0, "height": base.1},
            "actual": {"width": device.0, "height": device.1},
        }),
    ))
}

/// Records a frame from `capture --record` or `observe --capture --record`.
pub fn record_attach_frame(
    lock: &RecordingLock,
    request: AttachFrameRequest,
) -> LabResult<AttachFrameOutcome> {
    let mut session = open_session(lock)?;
    let device = DeviceFrame::decode(request)?;
    let arrival = arrive(
        &mut session,
        device.loaded,
        ArrivalMode::Device,
        &device.meta,
        &device.label,
    )?;
    save(&mut session)?;
    Ok(frame_recorded(
        &session.recording,
        arrival.opened,
        arrival.closed_step,
        arrival.step,
        arrival.frame,
    ))
}

/// A device frame decoded as `capture --record`'s is, with its provenance, before it arrives.
struct DeviceFrame {
    loaded: LoadedFrame,
    meta: FrameMeta,
    label: String,
}

impl DeviceFrame {
    fn decode(request: AttachFrameRequest) -> LabResult<Self> {
        let label = format!("{} frame", request.source);
        let loaded = decode_frame(request.png, &label)?;
        Ok(Self {
            loaded,
            meta: FrameMeta {
                source: request.source,
                runtime_artifact: request.runtime_artifact,
                capture_backend: request.capture_backend,
                freshness: request.freshness,
            },
            label,
        })
    }
}

/// The answer of frames that arrived as `capture --record`'s do: what the first of them opened
/// (`step_opened`) and closed (`closed_step`), and the step and frame of the last; one frame is
/// both.
fn frame_recorded(
    recording: &LabRecording,
    step_opened: bool,
    closed_step: Option<u32>,
    step: u32,
    frame: RecordedFrame,
) -> AttachFrameOutcome {
    AttachFrameOutcome {
        status: "frame_recorded".to_string(),
        record_id: recording.record_id.clone(),
        record_started_at_unix_ms: recording.record_started_at_unix_ms,
        step,
        step_opened,
        frame,
        closed_step,
    }
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
    let outcome = commit_recorded(lock, plan, request, false, false)?;
    if request.effect == ClickEffect::Indeterminate {
        return Err(click_indeterminate(plan.step, to_json(&outcome)));
    }
    if request.has_failure {
        return Err(click_performed_with_failure(plan.step, to_json(&outcome)));
    }
    Ok(outcome)
}

/// The Runtime outcome of a planned click recorded on its step, or
/// `record_append_failed_after_input` when it could not be recorded; #463 S1-1: with
/// `names_frames`, a press recorded by the command that makes it (`commit_click`), whose status is
/// `press_recorded` or `press_indeterminate` where `do --capture --record`'s is `click_recorded` or
/// `click_indeterminate`. Fix round 3: `sent_on_unstored_capture`, handed to `commit_click`, when
/// such a press was sent on a capture its command did not store.
fn commit_recorded(
    lock: &RecordingLock,
    plan: &ClickPlan,
    request: &CommitClickRequest,
    names_frames: bool,
    sent_on_unstored_capture: bool,
) -> LabResult<CommitClickOutcome> {
    let recorded = (|| -> LabResult<CommitClickOutcome> {
        let mut session = open_session(lock)?;
        let (click, performed) = commit_click(
            &mut session.recording,
            plan,
            request,
            names_frames,
            sent_on_unstored_capture,
        )?;
        save(&mut session)?;
        Ok(CommitClickOutcome {
            status: match (names_frames, performed) {
                (false, true) => "click_recorded",
                (false, false) => "click_indeterminate",
                (true, true) => "press_recorded",
                (true, false) => "press_indeterminate",
            }
            .to_string(),
            record_id: session.recording.record_id.clone(),
            record_started_at_unix_ms: session.recording.record_started_at_unix_ms,
            step: plan.step,
            step_closed: performed,
            click,
        })
    })();
    let effect = match request.effect {
        ClickEffect::Performed => "performed",
        ClickEffect::Indeterminate => "indeterminate",
    };
    recorded.map_err(|error| {
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
    })
}

/// The press of `step` has an indeterminate outcome: recorded, its step open, exit 4; `record` is
/// what was recorded.
fn click_indeterminate(step: u32, record: Value) -> LabError {
    LabError::new(
        LabErrorClass::DeviceInstance,
        "record_click_indeterminate",
        format!(
            "the click of step {step} has an indeterminate outcome; it is recorded and the step \
             stays open; check the screen, then `record mark --reopen-step {step}` to retry or \
             `record mark --close-step` to accept it"
        ),
        &["device"],
    )
    .with_details(json!({"record": record}))
}

/// The press of `step` was performed and the operation reported a failure: recorded, its step
/// closed and marked `needs_review`, exit 4; `record` is what was recorded.
fn click_performed_with_failure(step: u32, record: Value) -> LabError {
    LabError::new(
        LabErrorClass::DeviceInstance,
        "record_click_performed_with_failure",
        format!(
            "the click of step {step} was performed but the operation reported a failure; it is \
             recorded, the step is closed and marked needs_review"
        ),
        &["device"],
    )
    .with_details(json!({"record": record}))
}

/// The `actingcommand.lab-record-mark.v1` request in `text`, as `record mark --request` and
/// `--request-json` read it: text that does not decode into one (an unknown member at any level, a
/// member of the wrong type) is `validation_failed`. #463 L1: `do --capture --record
/// --mark-request` and `--mark-request-json` read theirs with it.
pub fn parse_mark_request(text: &str) -> LabResult<MarkRequest> {
    serde_json::from_str::<MarkRequest>(text)
        .map_err(|error| LabError::usage(format!("invalid record mark request: {error}")))
}

/// #463 L1 (`contracts/lab-recording.md`, "One call per step"): what the one-call form of `do
/// --capture --record` asks of its caller: the Runtime's press and capture, and the caller's
/// clock. actinglab gives the Runtime's ("`--record` on device commands"); a test gives fakes.
pub trait RecordStepPorts {
    /// Sends the planned input to the Runtime once, as `do --capture --record` sends it. `Ok`: the
    /// Runtime performed it, with or without a failure, or its outcome is unknown; what the
    /// recording takes of the answer. `Err`: it was not performed, or no answer came: the
    /// operation's own error, and nothing of the press is recorded.
    fn press(&mut self, plan: &ClickPlan) -> LabResult<CommitClickRequest>;

    /// Waits `duration` on the caller's clock.
    fn wait(&mut self, duration: Duration);

    /// Takes one frame through the Runtime, as `capture --record` takes it.
    fn capture(&mut self) -> LabResult<AttachFrameRequest>;
}

/// #463 L1: a one-call step planned under its recording lock: the mark request applied to the
/// open recording in memory by `record mark`'s own code and self-tested, and the press planned on
/// the open step by `do --capture --record`'s; nothing is written and nothing is sent until
/// [`PlannedStep::perform`].
pub struct PlannedStep {
    session: Session,
    mark: Option<MarkOutcome>,
    plan: ClickPlan,
    next_frame_after_ms: Option<u64>,
    next_frame_out: Option<PathBuf>,
}

/// #463 L1: plans a one-call step. In order: the next frame's bounds; the mark request's checks
/// and `record mark`'s work on the open recording, with its self-test on the step's live frames;
/// then the press, by `do --capture --record`'s rules (its point, `--tap`, `--seed`, a declared
/// drag or long press; the gestures it refuses stay refused). A refusal is the code of the check
/// that refused, `record mark`'s or `do --capture --record`'s, and nothing is written or sent. A
/// stopped recording takes no device command (`record_session_not_active`). Fix round 1: the
/// request is for the open step and nothing else ([`hold_to_open_step`]), and a refusal of a
/// request whose open step already holds its marks and no outcome of a press says what to look at
/// before the press is repeated, and how ([`repeat_without_mark`], fix round 2).
pub fn record_plan_step(
    lock: &RecordingLock,
    request: &RecordStepRequest,
    ocr: &LabOcr,
) -> LabResult<PlannedStep> {
    validate_next_frame(request)?;
    if let Some(mark) = &request.mark {
        validate_mark_request(mark)?;
    }
    let mut session = open_session(lock)?;
    let open = open_step(&session.recording);
    let unpressed = unpressed_open_step(&session.recording, open);
    let mark = request
        .mark
        .as_ref()
        .map(|mark| {
            mark_session(&mut session, mark, false, ocr)
                .map_err(|error| repeat_without_mark(error, unpressed.as_ref(), mark))
        })
        .transpose()?;
    let plan = plan_click(&session.recording, &request.click)?;
    if let (Some(mark_request), Some(outcome)) = (&request.mark, &mark) {
        hold_to_open_step(mark_request, open, outcome)?;
    }
    Ok(PlannedStep {
        session,
        mark,
        plan,
        next_frame_after_ms: request.next_frame_after_ms,
        next_frame_out: request.next_frame_out.clone(),
    })
}

impl PlannedStep {
    /// The press this step plans: the input the caller sends.
    pub fn plan(&self) -> &ClickPlan {
        &self.plan
    }

    /// #463 L1: performs the planned step under the same lock: the marks written once, as `record
    /// mark` writes them; the press sent through `ports` and recorded as `do --capture --record`
    /// records it; then, only after a performed press without a failure and only when asked for,
    /// the wait and the next frame, which arrives as `capture --record`'s does (it opens the next
    /// step). A press that was not performed leaves the marks recorded and answers the operation's
    /// own error; one performed with a failure or indeterminate answers as `do --capture --record`
    /// does; neither takes a next frame. A next frame that cannot be taken answers the capture's
    /// own error, and the press and its closed step stand: nothing is rolled back. Every error
    /// after the marks are written names them in its details (`mark`).
    pub fn perform(
        mut self,
        lock: &RecordingLock,
        ports: &mut impl RecordStepPorts,
    ) -> LabResult<RecordStepOutcome> {
        if self.mark.is_some() {
            save(&mut self.session)?;
        }
        let mark = self.mark;
        let pressed = ports
            .press(&self.plan)
            .map_err(|error| with_detail(error, "mark", mark.as_ref().map(to_json)))?;
        let click = record_commit_click(lock, &self.plan, &pressed)
            .map_err(|error| with_detail(error, "mark", mark.as_ref().map(to_json)))?;
        let Some(after_ms) = self.next_frame_after_ms else {
            return Ok(RecordStepOutcome {
                mark,
                click,
                next_frame: None,
            });
        };
        ports.wait(Duration::from_millis(after_ms));
        let (record, out) = next_frame(lock, ports, self.next_frame_out.as_deref())
            .map_err(|error| next_frame_failed(error, after_ms, &click, mark.as_ref()))?;
        Ok(RecordStepOutcome {
            mark,
            click,
            next_frame: Some(NextFrameOutcome {
                after_ms,
                out,
                record,
            }),
        })
    }
}

/// #463 L1: the next frame's wait is 0 to [`LAB_RECORDING_MAX_NEXT_FRAME_MS`] milliseconds, and its
/// file is named only with a wait: `validation_failed` otherwise, before anything is read.
fn validate_next_frame(request: &RecordStepRequest) -> LabResult<()> {
    match request.next_frame_after_ms {
        Some(after_ms) if after_ms > LAB_RECORDING_MAX_NEXT_FRAME_MS => {
            Err(LabError::usage(format!(
                "--next-frame-after-ms must be in 0..={LAB_RECORDING_MAX_NEXT_FRAME_MS}, got \
                 {after_ms}"
            ))
            .with_details(json!({
                "next_frame_after_ms": after_ms,
                "max_next_frame_after_ms": LAB_RECORDING_MAX_NEXT_FRAME_MS
            })))
        }
        None if request.next_frame_out.is_some() => Err(LabError::usage(
            "--next-frame-out names the file of the next frame: give --next-frame-after-ms with it",
        )),
        _ => Ok(()),
    }
}

/// #463 L1 fix round 1: the one-call form's mark request is for the open step and nothing else.
/// A step operation, a transition, a step other than the open one, or a frame file that opened or
/// replaced a step would make the press the click of a step the request did not mark, or a press
/// by the coordinates of a file: each is `validation_failed` (`member` names it), after the press
/// is planned and before anything is written or sent; `record mark` takes such a request.
fn hold_to_open_step(
    request: &MarkRequest,
    open: Option<u32>,
    outcome: &MarkOutcome,
) -> LabResult<()> {
    let (member, what) = if request.step_action.is_some() {
        ("step_action", "a step operation (step_action)".to_owned())
    } else if request.transition.is_some() {
        ("transition", "a transition (transition)".to_owned())
    } else if let Some(step) = request.step.filter(|step| Some(*step) != open) {
        (
            "step",
            format!("step {step} (step), which is not the open step"),
        )
    } else if outcome.frame.is_some() {
        (
            "frame",
            "a frame file (frame), which opened or replaced a step and would make the press \
             follow the coordinates of a file"
                .to_owned(),
        )
    } else {
        return Ok(());
    };
    let open_text = open.map_or_else(
        || "no step was open".to_owned(),
        |step| format!("the open step is {step}"),
    );
    Err(with_details(
        invalid(
            "validation_failed",
            format!(
                "the mark request of do --capture --record is for the open step and its press \
                 alone, and this one gives {what} ({open_text}): send it with record mark \
                 (ac_record_mark), then press with do --capture --record; nothing was written or \
                 pressed"
            ),
        ),
        json!({"member": member, "open_step": open}),
    ))
}

/// #463 L1 fix rounds 1 and 2: the open step while the recording holds no outcome of a press for
/// it: what a one-call step leaves whose press was not performed, whose device operation the
/// Runtime refused, or which was performed and could not be recorded
/// (`record_append_failed_after_input`). Its marks are written, and its click when the request
/// declared one; with `--tap-rect` or `--swipe` the click is declared only when the press is
/// recorded, so the step has none.
struct UnpressedStep {
    index: u32,
    /// The step declares its click (and no execution of it is on record).
    declares_click: bool,
    /// The step's own marks.
    marks: Vec<RecordedMark>,
}

fn unpressed_open_step(recording: &LabRecording, open: Option<u32>) -> Option<UnpressedStep> {
    let step = recording
        .steps
        .iter()
        .find(|step| Some(step.index) == open)?;
    let no_outcome = match &step.click {
        Some(click) => click.execution.is_none(),
        None => step.application.is_none(),
    };
    no_outcome.then(|| UnpressedStep {
        index: step.index,
        declares_click: step.click.is_some(),
        marks: step.marks.clone(),
    })
}

/// #463 L1 fix rounds 1 and 2 (the coordinator's rulings): the same one-call step sent again on
/// the open step is refused by `record mark`'s own check, `record_click_exists` for its declared
/// click, or `record_mark_id_conflict` when every mark the request adds is on the step already,
/// member for member ([`declares_mark`]; also with `--tap-rect` or `--swipe`, where the step has
/// no click yet), and nothing is pressed a second time. The refusal keeps its code and says that
/// the recording holds no outcome of a press, which can mean that the press was not performed or
/// that it was performed and could not be recorded: look at the screen, or read `record status`,
/// and only while the step's own page is still shown send the same call without the mark request.
fn repeat_without_mark(
    mut error: LabError,
    unpressed: Option<&UnpressedStep>,
    request: &MarkRequest,
) -> LabError {
    let Some(step) = unpressed else {
        return error;
    };
    let on_open_step = request.step_action.is_none()
        && request.transition.is_none()
        && request.step.is_none_or(|index| index == step.index);
    let again = on_open_step
        && match error.code.as_str() {
            "record_click_exists" => step.declares_click,
            "record_mark_id_conflict" => {
                // #463 S2-5: a recognize box is the template mark it stands for on the open step.
                let add = expanded_add(request, step.index);
                !add.is_empty()
                    && add
                        .iter()
                        .all(|spec| step.marks.iter().any(|mark| declares_mark(spec, mark)))
            }
            _ => false,
        };
    if !again {
        return error;
    }
    let (holds, send) = if step.declares_click {
        (
            "its marks and its click",
            "the same call without the mark request (do --capture --record without \
             --mark-request and --mark-request-json; MCP: ac_lab_do without mark, on the Lab's MCP \
             server ac_do without mark_request_json and mark_request)",
        )
    } else {
        (
            "its marks and no click yet (--tap-rect or --swipe declares it when the press is \
             recorded)",
            "the same call without the mark request, its --tap-rect or --swipe kept (do --capture \
             --record without --mark-request and --mark-request-json; MCP: ac_lab_do with tap_rect \
             or swipe and without mark, on the Lab's MCP server ac_do with tap_rect or swipe and \
             without mark_request_json and mark_request)",
        )
    };
    error.message = format!(
        "{}; step {} already holds {holds}, and the recording holds no outcome of a press for it: \
         the press was not performed, or it was performed and could not be recorded (an earlier \
         answer record_append_failed_after_input means it was performed). Look at the screen \
         first, with observe --capture without --record (MCP: ac_lab_observe with capture and \
         without record, on both servers), or read record status (MCP: ac_record_status): only \
         while the screen still shows step {}'s own page, send {send}",
        error.message, step.index, step.index
    );
    error
}

/// #463 L1: the next frame after a performed press, as `capture --record` takes it: the open
/// recording must accept a device frame, then the capture, its PNG written to `out` when that is
/// named, then the frame recorded. Answers the recorded frame and the file written.
fn next_frame(
    lock: &RecordingLock,
    ports: &mut impl RecordStepPorts,
    out: Option<&Path>,
) -> LabResult<(AttachFrameOutcome, Option<String>)> {
    record_frame_preflight(lock)?;
    let frame = ports.capture()?;
    let written = out
        .map(|path| write_next_frame(path, &frame.png))
        .transpose()?;
    Ok((record_attach_frame(lock, frame)?, written))
}

/// The next frame's PNG written to `path`, its directory made first, as `capture --record --out`
/// writes its frame (a failure is a device error there too).
fn write_next_frame(path: &Path, png: &[u8]) -> LabResult<String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            LabError::device(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    std::fs::write(path, png).map_err(|error| {
        LabError::device(format!("failed to write {}: {error}", path.display()))
    })?;
    Ok(path.display().to_string())
}

/// #463 L1: the error of a next frame that could not be taken: the capture's own code and class,
/// its message saying that the press stands, its details naming the recorded press (`record`), the
/// wait (`next_frame`) and the marks (`mark`). Fix round 1: `record` and `next_frame` lead the
/// details, before the capture's own members, so that a copy of the details cut at a bound (the
/// MCP server's `raw_text` keeps their first 4096 bytes) still tells this press from one that was
/// not performed, whose details have neither.
fn next_frame_failed(
    mut error: LabError,
    after_ms: u64,
    click: &CommitClickOutcome,
    mark: Option<&MarkOutcome>,
) -> LabError {
    error.message = format!(
        "{}; the press of step {} was performed and recorded and the step is closed: take the \
         next frame with capture --record or observe --capture --record (MCP: ac_lab_observe \
         with capture and record), do not press again",
        error.message, click.step
    );
    let error = with_leading_details(
        error,
        [
            ("record", to_json(click)),
            ("next_frame", json!({"after_ms": after_ms, "taken": false})),
        ],
    );
    with_detail(error, "mark", mark.map(to_json))
}

/// `error` whose details begin with `leading`, then keep the members they had (other details move
/// to `details`); a member of `leading` replaces one of the same name.
fn with_leading_details<const N: usize>(
    mut error: LabError,
    leading: [(&str, Value); N],
) -> LabError {
    let mut details = leading
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect::<Map<String, Value>>();
    match error.details.take() {
        Some(Value::Object(members)) => {
            for (name, value) in members {
                details.entry(name).or_insert(value);
            }
        }
        Some(other) => {
            details.entry("details".to_owned()).or_insert(other);
        }
        None => {}
    }
    error.with_details(Value::Object(details))
}

/// `error` with `value`, when there is one, as the member `name` of its details: the members of
/// object details are kept, other details move to `details`.
fn with_detail(mut error: LabError, name: &str, value: Option<Value>) -> LabError {
    let Some(value) = value else {
        return error;
    };
    let mut details = match error.details.take() {
        Some(Value::Object(details)) => details,
        Some(other) => Map::from_iter([("details".to_owned(), other)]),
        None => Map::new(),
    };
    details.insert(name.to_owned(), value);
    error.with_details(Value::Object(details))
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): what recording a press by
/// the command that makes it asks of its caller beside the press: the caller's clock and one frame
/// through the Runtime, as `capture --record` takes it (a part of what [`RecordStepPorts`] asks,
/// and, #463 S1-2, the clock's readings). actinglab gives the Runtime's; a test gives fakes.
pub trait RecordFramePorts {
    /// Waits `duration` on the caller's clock.
    fn wait(&mut self, duration: Duration);

    /// Takes one frame through the Runtime, as `capture --record` takes it; fix round 2: with the
    /// reference of the frame the Runtime committed for it, which a press planned on it names.
    fn capture(&mut self) -> LabResult<CapturedFrame>;

    /// #463 S1-2: the time on the caller's clock, since an origin of the caller's (only the
    /// difference of two readings is used): when each capture after a press starts, so that the
    /// next starts `interval_ms` after it, or at once when it took longer.
    fn elapsed(&mut self) -> Duration;
}

/// #463 S1-1: the frame a press is sent on, under the recording lock. Fix round 2: every press
/// recorded by the command that makes it is sent on a frame captured immediately before it, so
/// one frame is always taken here, and what is kept of it depends on the open step:
/// - no step open (a new recording, or the frame after the previous press could not be taken):
///   it arrives as `capture --record`'s does and opens the step the press is planned on;
/// - an open step without marks: it arrives as `capture --record`'s does on that step, so it is
///   the step's live primary frame and the older one stays, superseded;
/// - an open step with marks (made on its stored frame): it is not stored, and the stored frame
///   stands; it must be of the recording's size (`record_frame_size_mismatch`), so that a point
///   inside the recording's frames is a pixel of it.
///
/// A capture whose observation carries no frame id cannot be pressed on: `device_error`, before
/// anything of it is stored. Answers the Runtime's reference of the frame, which the press is sent
/// with, and its arrival (`None` when it was not stored).
pub fn record_before_frame(
    lock: &RecordingLock,
    ports: &mut impl RecordFramePorts,
) -> LabResult<BeforeCapture> {
    let (stores, coordinate_space) = {
        let session = open_session(lock)?;
        let recording = &session.recording;
        let stores = open_step(recording)
            .is_none_or(|index| !step_ref(recording, index).is_some_and(has_marks));
        (stores, recording.coordinate_space)
    };
    let captured = ports.capture()?;
    // The fix round after the train's review: this frame is taken before a press or (#463 S1-3) an
    // application operation, and the texts name both.
    let input_frame = captured.input_frame.ok_or_else(|| {
        LabError::device(
            "the Runtime's capture before the press or application operation carries no frame \
             id, so nothing can name the frame it is planned on; nothing was pressed, sent or \
             recorded",
        )
    })?;
    if stores {
        let arrived = record_attach_frame(lock, captured.frame)?;
        return Ok(BeforeCapture {
            input_frame,
            arrived: Some(arrived),
        });
    }
    let label = "the frame before the press or application operation";
    check_frame_size(
        coordinate_space,
        &decode_frame(captured.frame.png, label)?,
        label,
    )?;
    Ok(BeforeCapture {
        input_frame,
        arrived: None,
    })
}

/// #463 S1-2 (`contracts/lab-recording.md`, "Recording while playing"): the time from the start of
/// one capture after a press to the start of the next when none is given (`--interval-ms`), and
/// its range.
const AFTER_INTERVAL_MS: u64 = 100;
const AFTER_MIN_INTERVAL_MS: u64 = 20;
const AFTER_MAX_INTERVAL_MS: u64 = 1_000;

/// #463 S1-2: how long the captures after a press must show one screen when none is given
/// (`--still-ms`; 0: no stillness test), and the most it may be.
const AFTER_STILL_MS: u64 = 300;
const AFTER_MAX_STILL_MS: u64 = 5_000;

/// #463 S1-2: the most intervals the bound of the stillness test may hold (`still_max_ms /
/// interval_ms`), so that a press takes at most one capture more than this after it.
const AFTER_MAX_INTERVALS: u64 = 120;

/// #463 S1-2: the warning of the answer's `record.warnings` when the stillness test reached its
/// bound without a still screen.
const AFTER_NOT_STILL: &str = "after_not_still";

/// #463 S1-2 and #463 S1-3 (the train of stage one, which joins S1-2's captures after a press with
/// S1-3's application operations): how the screen after an effect is to be taken, checked before
/// anything is captured, pressed or sent: the wait `after_ms` 0 to 60000, `interval_ms` 20 to 1000,
/// `still_ms` 0 to 5000 and `still_max_ms` 0 to 60000 (`validation_failed`, `details{argument,
/// reason: "range", value, min, max}`), and at most 120 intervals in the bound: `still_max_ms` at
/// most 120 times `interval_ms`, a range that depends on the interval (fix round 3:
/// `details{argument: "still_max_ms", reason: "range", value, min: 0, max: 120 times interval_ms,
/// interval_ms}`), so that an effect takes at most 121 captures after it. The fix round after the
/// train's review: one function checks the four, `after_ms` with the keys of the other three.
/// [`record_press`] checks a press's so, [`record_after_frame`] the screen after it is handed, and
/// `session app <verb> --record` its operation's before it sends the operation.
pub fn record_after_capture_check(after: &AfterCapture) -> LabResult<()> {
    let interval_ms = after.interval_ms.unwrap_or(AFTER_INTERVAL_MS);
    for (argument, flag, value, min, max) in [
        (
            "after_ms",
            "--after-ms",
            after.after_ms,
            0,
            LAB_RECORDING_MAX_NEXT_FRAME_MS,
        ),
        (
            "interval_ms",
            "--interval-ms",
            interval_ms,
            AFTER_MIN_INTERVAL_MS,
            AFTER_MAX_INTERVAL_MS,
        ),
        (
            "still_ms",
            "--still-ms",
            after.still_ms.unwrap_or(AFTER_STILL_MS),
            0,
            AFTER_MAX_STILL_MS,
        ),
        (
            "still_max_ms",
            "--still-max-ms",
            after.still_max_ms,
            0,
            LAB_RECORDING_MAX_NEXT_FRAME_MS,
        ),
    ] {
        if !(min..=max).contains(&value) {
            return Err(LabError::usage(format!(
                "{flag} (MCP {argument}) must be in {min}..={max}, got {value}"
            ))
            .with_details(json!({
                "argument": argument,
                "reason": "range",
                "value": value,
                "min": min,
                "max": max
            })));
        }
    }
    let max = AFTER_MAX_INTERVALS * interval_ms;
    if after.still_max_ms > max {
        return Err(LabError::usage(format!(
            "--still-max-ms (MCP still_max_ms) must be in 0..={max} at --interval-ms (MCP \
             interval_ms) {interval_ms}, at most {AFTER_MAX_INTERVALS} intervals, so that a press \
             or an application operation takes at most {} captures after it; got {}",
            AFTER_MAX_INTERVALS + 1,
            after.still_max_ms
        ))
        .with_details(json!({
            "argument": "still_max_ms",
            "reason": "range",
            "value": after.still_max_ms,
            "min": 0,
            "max": max,
            "interval_ms": interval_ms
        })));
    }
    Ok(())
}

/// #463 S1-2: the words an answer's `record.warnings` holds for how the screen after its effect was
/// taken (`record`): `after_not_still` when the stillness test reached its bound without a still
/// screen, none otherwise. #463 S1-3 (the train of stage one): a press's answer ([`record_press`])
/// and an application operation's (`session app <verb> --record`) read them here.
pub fn record_after_warnings(record: &AfterCaptureRecord) -> Vec<String> {
    if record.still == Some(false) {
        vec![AFTER_NOT_STILL.to_owned()]
    } else {
        Vec::new()
    }
}

/// #463 S1-2: the frame-change digest of a whole frame (`frame_change_digest`, called as the
/// kernel's still waits call it); a frame it cannot digest is `record_frame_unreadable`. #463 S2-5:
/// `record mark` digests a step's live frame and its captures with it too (`steps.rs`).
pub(crate) fn change_digest(frame: &LoadedFrame, label: &str) -> LabResult<ColorDigest> {
    frame_change_digest(&frame.scene, None).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to digest {label} for the frame-change measure: {error}"),
        )
    })
}

/// #463 S1-2 (fix round 3, the coordinator's ruling: "still" does not depend on how long a capture
/// takes): whether the screen has not changed for at least `still_for`. `taken` holds the start
/// and digest of each capture, oldest first; its last is the newest. The run is the newest and the
/// unbroken run of captures before it that each differ from the newest by at most the Runtime's
/// one frame-change bound (`frame_change_milli` at most `FRAME_CHANGE_MAX_MILLI`); the screen is
/// still when the run holds at least one earlier capture and the newest started at least
/// `still_for` after the run's first. Digests that cannot be compared are
/// `record_frame_unreadable`. #463 S2-5: `record mark` asks it whether a capture of a step shows
/// the state of its live frame (`steps.rs`, `frame_unchanged`).
pub(crate) fn unchanged_for(
    taken: &[(Duration, ColorDigest)],
    still_for: Duration,
) -> LabResult<bool> {
    let Some(((newest_start, newest), earlier)) = taken.split_last() else {
        return Ok(false);
    };
    for (start, digest) in earlier.iter().rev() {
        let milli = frame_change_milli(digest, newest).map_err(|error| {
            invalid(
                "record_frame_unreadable",
                format!("failed to compare two frames by the frame-change measure: {error}"),
            )
        })?;
        if milli > FRAME_CHANGE_MAX_MILLI {
            return Ok(false);
        }
        if newest_start.saturating_sub(*start) >= still_for {
            return Ok(true);
        }
    }
    Ok(false)
}

/// #463 S1-2: one capture of the screen after a press, arrived on `session` as a capture of the
/// press's after loop (`arrive_after_press`), with its frame-change digest when the loop tests
/// stillness (`digests`). The loop writes its captures when it ends and reads none of them again,
/// so their decoded pixels leave the session's cache (its pending writes keep the PNG).
fn take_after_capture(
    session: &mut Session,
    ports: &mut impl RecordFramePorts,
    digests: bool,
) -> LabResult<(Arrival, Option<ColorDigest>)> {
    let device = DeviceFrame::decode(ports.capture()?.frame)?;
    let digest = if digests {
        Some(change_digest(&device.loaded, &device.label)?)
    } else {
        None
    };
    let arrival = arrive_after_press(session, device.loaded, &device.meta, &device.label)?;
    session.cache.remove(&arrival.frame.sha256);
    Ok((arrival, digest))
}

/// Whole milliseconds of `duration`.
fn whole_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// #463 S1-2: the timing of an after loop, its defaults filled in: from the start of one capture
/// to the start of the next (`interval_ms`), how long the screen must not change (`still_ms`), the
/// bound after the first capture (`still_max_ms`), and (fix round 3) the most captures the loop
/// takes, `1 + still_max_ms / interval_ms`, whatever the caller's clock says.
struct AfterTiming {
    interval: Duration,
    still_for: Duration,
    bound: Duration,
    most: u32,
}

/// #463 S1-2: what an after loop has taken so far: when its first capture started, the start,
/// step and frame of the last, how many, and the start and frame-change digest of each (when the
/// loop tests stillness), oldest first.
struct AfterLoop {
    first_start: Duration,
    last: (Duration, u32, RecordedFrame),
    captures: u32,
    digests: Vec<(Duration, ColorDigest)>,
}

/// #463 S1-2: the captures after the first, until the screen is still (`true`) or the bound is
/// reached (`false`): each starts `interval` after the start of the one before it, or at once when
/// that one took longer; the loop ends when the screen has not changed for `still_for`
/// ([`unchanged_for`]), when the next capture would start more than `bound` after the first, or
/// (fix round 3) when `most` were taken, so that a caller's clock that does not advance never holds
/// it. A capture that cannot be taken or compared answers its error, and `taken` keeps every
/// capture that arrived before it.
fn capture_until_still(
    session: &mut Session,
    ports: &mut impl RecordFramePorts,
    timing: &AfterTiming,
    taken: &mut AfterLoop,
) -> LabResult<bool> {
    loop {
        let now = ports.elapsed();
        let next = (taken.last.0 + timing.interval).max(now);
        if taken.captures >= timing.most || next.saturating_sub(taken.first_start) > timing.bound {
            return Ok(false);
        }
        if next > now {
            ports.wait(next - now);
        }
        let start = ports.elapsed();
        let (arrival, digest) = take_after_capture(session, ports, true)?;
        taken.captures += 1;
        taken.last = (start, arrival.step, arrival.frame);
        taken.digests.extend(digest.map(|digest| (start, digest)));
        if unchanged_for(&taken.digests, timing.still_for)? {
            return Ok(true);
        }
    }
}

/// #463 S1-2: what an after loop kept: the answer of its last capture's arrival, how the screen
/// after was taken, and (fix round 3) the error of a capture after the first that could not be
/// taken (`stopped_by`): the loop did not finish (`record.unfinished`). The fix round after the
/// train's review: public, the typed result [`record_after_frame`] answers, so that `session app
/// <verb> --record` builds the operation as recorded from it as [`record_press`] builds a press;
/// `stopped_by` is the capture's own error as `after_loop_stopped` words it (the captures after
/// the effect stopped, what was kept, its details beginning with `after: {after_ms, taken: true,
/// captures}`), to be answered once what the loop kept is recorded.
#[derive(Debug)]
pub struct AfterTaken {
    pub frame: AttachFrameOutcome,
    pub record: AfterCaptureRecord,
    pub stopped_by: Option<LabError>,
}

/// The fix round after the train's review: what the screen after is taken after, the noun the
/// texts of [`capture_after`] name it by.
#[derive(Debug, Clone, Copy)]
enum AfterEffect {
    /// A press, `tap`, `swipe` or `long-tap --record` ([`record_press`]).
    Press,
    /// #463 S1-3: an application operation, `session app <verb> --record`
    /// ([`record_after_frame`]).
    Application,
}

impl AfterEffect {
    const fn noun(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Application => "application operation",
        }
    }
}

/// #463 S1-3 (the train of stage one): the screen after a performed application operation, under
/// the recording lock, the after loop of a press (`capture_after`; [`record_press`] runs it for a
/// press itself). The fix round after the train's review: it answers the loop's typed result,
/// [`AfterTaken`], whatever the loop kept, its `stopped_by` the error of a capture after the first
/// that stopped it (worded for an application operation); `Err` when nothing was kept: the four
/// parameters out of their ranges (checked first, [`record_after_capture_check`]) or a first
/// capture that could not be taken.
pub fn record_after_frame(
    lock: &RecordingLock,
    ports: &mut impl RecordFramePorts,
    after: &AfterCapture,
) -> LabResult<AfterTaken> {
    capture_after(lock, ports, after, AfterEffect::Application)
}

/// #463 S1-1: the frame after a performed press, under the recording lock. #463 S1-2
/// (`contracts/lab-recording.md`, "Recording while playing"): the parameters are checked first
/// (fix round 3: here too, so that the bound holds wherever the loop is entered; the fix round
/// after the train's review: the four, [`record_after_capture_check`]), then the screen is captured
/// until it is still, on the caller's clock
/// (`RecordFramePorts::elapsed` and `wait`): the first capture starts `after_ms` after the press,
/// then one every `interval_ms` (default 100), counted from the start of one capture to the start
/// of the next (one that took longer is followed at once). Fix round 3: the screen is still when it
/// has not changed for at least `still_ms` (default 300), however long a capture takes: the newest
/// capture and the unbroken run of captures before it that each differ from it by at most the
/// Runtime's one frame-change bound hold at least one earlier capture, and the newest started at
/// least `still_ms` after the run's first (`unchanged_for`). The loop ends at the first still
/// capture (`still` true), or when the next capture would start more than `still_max_ms` after the
/// first or `1 + still_max_ms / interval_ms` captures were taken (`still` false); with `still_ms`
/// 0 the first capture ends it untested (`still` `None`). Before the first capture the recording
/// must take a device frame, as `capture --record`'s; every capture arrives as one: the first opens
/// the next step, each later one supersedes the one before it (a capture of the same bytes is the
/// same file, its own entry), and the last is that step's live primary frame. When the loop ends,
/// the press's execution names the last as its `after_frame` (the one write, `name_after_frame`)
/// and keeps `after_capture {after_ms, interval_ms, still_ms, still_max_ms, still, waited_ms,
/// captures, kept}` (`waited_ms` from the press to the start of the last capture, `kept` the
/// captures beside it), all written at once. A first capture that fails answers its own error and
/// stores nothing. Fix round 3: a later capture that fails ends the loop unfinished: what it took
/// is written as above, `after_capture` with `still` `None` and `unfinished` true, and the
/// capture's own error is kept for the caller to answer, its details beginning with `after:
/// {after_ms, taken: true, captures}` ([`after_loop_stopped`], naming `effect`). What the loop
/// kept, and that error (`AfterTaken::stopped_by`); `Err` when nothing was kept.
fn capture_after(
    lock: &RecordingLock,
    ports: &mut impl RecordFramePorts,
    after: &AfterCapture,
    effect: AfterEffect,
) -> LabResult<AfterTaken> {
    record_after_capture_check(after)?;
    let interval_ms = after.interval_ms.unwrap_or(AFTER_INTERVAL_MS);
    let still_ms = after.still_ms.unwrap_or(AFTER_STILL_MS);
    let timing = AfterTiming {
        interval: Duration::from_millis(interval_ms),
        still_for: Duration::from_millis(still_ms),
        bound: Duration::from_millis(after.still_max_ms),
        most: u32::try_from(1 + after.still_max_ms / interval_ms).unwrap_or(u32::MAX),
    };
    let pressed_at = ports.elapsed();
    ports.wait(Duration::from_millis(after.after_ms));
    let mut session = open_session(lock)?;
    check_device_arrival(&session.recording)?;
    let first_start = ports.elapsed();
    let (first, first_digest) = take_after_capture(&mut session, ports, still_ms > 0)?;
    let pressed = first.pressed_step.ok_or_else(|| {
        blocked(
            "record_step_not_found",
            format!(
                "the frame after the {} opened step {} but is the after frame of no press or \
                 application operation recorded by its command",
                effect.noun(),
                first.step
            ),
        )
    })?;
    let (opened, closed_step) = (first.opened, first.closed_step);
    let mut taken = AfterLoop {
        first_start,
        last: (first_start, first.step, first.frame),
        captures: 1,
        digests: first_digest
            .map(|digest| (first_start, digest))
            .into_iter()
            .collect(),
    };
    let (still, stopped_by) = if still_ms == 0 {
        (None, None)
    } else {
        match capture_until_still(&mut session, ports, &timing, &mut taken) {
            Ok(still) => (Some(still), None),
            Err(error) => (None, Some(error)),
        }
    };
    let AfterLoop {
        last: (last_start, step, frame),
        captures,
        ..
    } = taken;
    let record = AfterCaptureRecord {
        after_ms: after.after_ms,
        interval_ms: Some(interval_ms),
        still_ms: Some(still_ms),
        still_max_ms: after.still_max_ms,
        still,
        waited_ms: whole_ms(last_start.saturating_sub(pressed_at)),
        captures,
        kept: captures - 1,
        unfinished: stopped_by.is_some(),
    };
    name_after_frame(&mut session.recording, pressed, &frame.frame_id)?;
    note_after_capture(&mut session.recording, pressed, &record)?;
    save(&mut session)?;
    let stopped_by = stopped_by.map(|error| after_loop_stopped(error, &record, effect));
    Ok(AfterTaken {
        frame: frame_recorded(&session.recording, opened, closed_step, step, frame),
        record,
        stopped_by,
    })
}

/// #463 S1-2 fix round 3: the error of an after loop that a capture after its first stopped: the
/// capture's own code and class, its message saying that the captures after the effect (the fix
/// round after the train's review: the press or the application operation, `effect`) stopped
/// there and that the captures taken before it are kept, the last the frame after, its details
/// beginning with `after: {after_ms, taken: true, captures}`, then the capture's own members.
fn after_loop_stopped(
    mut error: LabError,
    record: &AfterCaptureRecord,
    effect: AfterEffect,
) -> LabError {
    error.message = format!(
        "{}; the captures after the {} stopped there: the {} taken before it are kept, the last \
         its frame after, and the loop did not finish (after_capture.unfinished)",
        error.message,
        effect.noun(),
        record.captures
    );
    with_leading_details(
        error,
        [(
            "after",
            json!({
                "after_ms": record.after_ms,
                "taken": true,
                "captures": record.captures
            }),
        )],
    )
}

/// #463 S1-1 (`contracts/lab-recording.md`, "Recording while playing"): `tap`, `swipe` or
/// `long-tap --record`, a press recorded by the command that makes it, under the recording lock.
/// In order: the wait's bound (`validation_failed`, `details{argument: "after_ms", reason:
/// "range"}`); on an open step, the press planned on it as it stands, so that every refusal of
/// the step or the point comes before anything is captured; the frame the press is sent on
/// ([`record_before_frame`], fix round 2: always captured, immediately before the press); the
/// press planned on the open step (`plan_press`: a tap or a long press declares a box of one pixel
/// at its point, a swipe is today's recorded swipe; every refusal before anything is pressed); the
/// press sent once through `press` with the Runtime's reference of that frame, whose `Err` (not
/// performed) is answered as it is and records no press (the capture before it stays as it was
/// kept); fix round 3: except one `Err` marked [`record_press_frame_unavailable`] (the Runtime no
/// longer held the frame, nothing was sent), after which the frame is captured once more as the
/// first was (a stored first one superseded by the second; on a step with marks neither stored),
/// the press planned again and sent once more with the new reference, its answer's
/// `before.captured_twice`, and a second `Err` of any kind answered as it is, unmarked; its outcome
/// recorded, the execution naming the frame before (and `sent_on_unstored_capture` when that
/// capture was not stored); then, only after a press performed without a failure, the frame after
/// (`capture_after`). An indeterminate press is `record_click_indeterminate` and a press
/// performed with a failure `record_click_performed_with_failure`, each with the press as recorded
/// in `record` and no frame after. A frame after that cannot be taken answers its own error and
/// the press and its closed step stand: its details begin with `record` (the press as recorded)
/// and `after: {after_ms, taken: false}`. #463 S1-2: with the wait's bound, the rest of how the
/// screen after is taken ([`record_after_capture_check`]); a stillness test that reached its bound
/// without a still screen puts `after_not_still` in the answer's `warnings`. Fix round 3: a capture
/// after the first that cannot be taken leaves the captures taken before it kept and recorded (the
/// last the frame after, `after_capture.unfinished`) and answers its own error, its details
/// beginning with `record` and `after: {after_ms, taken: true, captures}`.
pub fn record_press(
    lock: &RecordingLock,
    request: &RecordPressRequest,
    press: &mut impl FnMut(&ClickPlan, InputFrameReference) -> LabResult<CommitClickRequest>,
    ports: &mut impl RecordFramePorts,
) -> LabResult<RecordPressOutcome> {
    let after_ms = request.after.after_ms;
    // The train of stage one: S1-1's check of the wait and S1-2's of the rest, in that order, in
    // the one function `session app <verb> --record` calls too (the fix round after the train's
    // review: one check of the four).
    record_after_capture_check(&request.after)?;
    {
        let session = open_session(lock)?;
        if open_step(&session.recording).is_some() {
            plan_press(&session.recording, &request.click, request.point_press)?;
        }
    }
    let taken = record_before_frame(lock, ports)?;
    let (plan, before) = planned_press(lock, request, taken.arrived.is_some(), false)?;
    let (plan, before, pressed) = match press(&plan, taken.input_frame) {
        // Fix round 3: the Runtime no longer held the frame the press named, and nothing was
        // sent: one capture more, as the first was taken, and the press sent once more on it.
        // A second refusal of any kind is answered as it is.
        Err(refusal) if press_frame_was_unavailable(&refusal) => {
            let again = record_before_frame(lock, ports)?;
            let (plan, before) = planned_press(lock, request, again.arrived.is_some(), true)?;
            let pressed = press(&plan, again.input_frame).map_err(without_frame_unavailable)?;
            (plan, before, pressed)
        }
        answered => (plan, before, answered?),
    };
    let click = commit_recorded(lock, &plan, &pressed, true, before.sent_on_unstored_capture)?;
    let mut outcome = RecordPressOutcome {
        click,
        before,
        after: None,
        warnings: Vec::new(),
    };
    if pressed.effect == ClickEffect::Indeterminate {
        return Err(click_indeterminate(plan.step, to_json(&outcome)));
    }
    if pressed.has_failure {
        return Err(click_performed_with_failure(plan.step, to_json(&outcome)));
    }
    let AfterTaken {
        frame,
        record,
        stopped_by,
    } = capture_after(lock, ports, &request.after, AfterEffect::Press)
        .map_err(|error| after_frame_failed(error, &outcome, after_ms))?;
    // #463 S1-2: a stillness test that reached its bound without a still screen says so.
    outcome.warnings.extend(record_after_warnings(&record));
    // The click as recorded now: its execution names the frame after it and how it was taken.
    if let Some(execution) = outcome.click.click.execution.as_mut() {
        execution.after_frame = Some(frame.frame.frame_id.clone());
        execution.after_capture = Some(record.clone());
    }
    outcome.after = Some(AfterFrame {
        step: frame.step,
        frame_id: frame.frame.frame_id,
        path: frame.frame.path,
        sha256: frame.frame.sha256,
        record,
    });
    // #463 S1-2 fix round 3: a capture after the first that failed is answered, loud, once what
    // the loop took is kept and recorded.
    match stopped_by {
        None => Ok(outcome),
        Some(error) => Err(after_loop_stopped_press(error, &outcome)),
    }
}

/// #463 S1-1: the press of [`record_press`] planned on the open step after the capture before it,
/// and the frame it is committed on, the step's live primary frame (`before`): `stored` when that
/// capture was stored (the command captured the frame `before` names; otherwise the press is sent
/// on a capture that was not stored); fix round 3: `captured_twice` when that capture is the
/// second, taken after the Runtime no longer held the frame of the first.
fn planned_press(
    lock: &RecordingLock,
    request: &RecordPressRequest,
    stored: bool,
    captured_twice: bool,
) -> LabResult<(ClickPlan, BeforeFrame)> {
    let session = open_session(lock)?;
    let plan = plan_press(&session.recording, &request.click, request.point_press)?;
    let before = session
        .recording
        .steps
        .iter()
        .find(|step| step.index == plan.step)
        .and_then(live_primary)
        .map(|frame| BeforeFrame {
            frame_id: frame.frame_id.clone(),
            path: frame.path.clone(),
            sha256: frame.sha256.clone(),
            refreshed: stored,
            sent_on_unstored_capture: !stored,
            captured_twice,
        })
        .ok_or_else(|| {
            blocked(
                "record_step_frame_missing",
                format!(
                    "step {} has no frame to press on; record its frame with capture --record \
                     first",
                    plan.step
                ),
            )
        })?;
    Ok((plan, before))
}

/// #463 S1-1 fix round 3: the member of a press's refusal that [`record_press_frame_unavailable`]
/// adds, and that [`record_press`] reads and takes away again before anything is answered.
const PRESS_FRAME_UNAVAILABLE: &str = "press_frame_unavailable";

/// #463 S1-1 fix round 3 (`contracts/lab-recording.md`, "Recording while playing"): `refusal`, the
/// `Err` of [`record_press`]'s `press` when the Runtime refused the input before sending anything
/// because it no longer held the frame the input names (the caller tells that refusal from every
/// other, in one place, by what its Runtime client hands it), marked so that `record_press`
/// captures once more and sends the press again on the new capture, once. The mark is a member of
/// the details (object details keep theirs; others move to `details`, as `with_detail` moves
/// them), and never reaches an answer.
pub fn record_press_frame_unavailable(refusal: LabError) -> LabError {
    with_detail(refusal, PRESS_FRAME_UNAVAILABLE, Some(Value::Bool(true)))
}

/// Whether `refusal` carries the mark of [`record_press_frame_unavailable`].
fn press_frame_was_unavailable(refusal: &LabError) -> bool {
    matches!(
        refusal
            .details
            .as_ref()
            .and_then(|details| details.get(PRESS_FRAME_UNAVAILABLE)),
        Some(Value::Bool(true))
    )
}

/// `refusal` as its caller answered it, without the mark of [`record_press_frame_unavailable`]:
/// the member taken from its details, and details that held nothing else none.
fn without_frame_unavailable(mut refusal: LabError) -> LabError {
    let emptied = match refusal.details.as_mut() {
        Some(Value::Object(details)) => {
            details.remove(PRESS_FRAME_UNAVAILABLE).is_some() && details.is_empty()
        }
        _ => false,
    };
    if emptied {
        refusal.details = None;
    }
    refusal
}

/// #463 S1-1: the input `tap`, `swipe` or `long-tap --record` sends for a planned press, as the
/// kernel presses the click the recording generates for it: a tap at its point; a long press as
/// one long tap at its point over its duration; a drag (a recorded swipe) as one swipe from its
/// point to its end point over its duration. No such plan carries another gesture: one would be
/// `record_gesture_execution_unsupported`, as `do --capture --record` refuses it.
pub fn record_press_input(plan: &ClickPlan) -> LabResult<InputAction> {
    match (plan.gesture, plan.to_point) {
        (None, _) => Ok(InputAction::Tap {
            x: plan.point.x,
            y: plan.point.y,
        }),
        (Some(ClickGesture::LongPress { duration_ms }), _) => Ok(InputAction::LongTap {
            x: plan.point.x,
            y: plan.point.y,
            duration_ms,
        }),
        (Some(ClickGesture::Drag { duration_ms, .. }), Some(to)) => Ok(InputAction::Swipe {
            x1: plan.point.x,
            y1: plan.point.y,
            x2: to.x,
            y2: to.y,
            duration_ms,
        }),
        (Some(ClickGesture::Drag { .. }), None) => Err(LabError::usage(format!(
            "the planned drag of step {} names no end point",
            plan.step
        ))),
        (
            Some(
                gesture @ (ClickGesture::SingleTouchDragWithVerticalBrakeV1 { .. }
                | ClickGesture::SingleTouchPathDragV1 { .. }),
            ),
            _,
        ) => Err(LabError::safety_blocked(
            LabCode::RecordGestureExecutionUnsupported.as_str(),
            format!(
                "the {} of step {} is not pressed by a device command",
                gesture.kind(),
                plan.step
            ),
            &["session_record"],
        )
        .with_details(json!({"step": plan.step, "gesture": gesture}))),
    }
}

/// #463 S1-1: the error of a frame after a press that could not be taken (#463 S1-2: nothing of
/// the after loop was kept): the capture's own code and class, its message saying that the press
/// stands, its details beginning with `record` (the press as recorded) and `after: {after_ms,
/// taken: false}`, then the capture's own members.
fn after_frame_failed(
    mut error: LabError,
    outcome: &RecordPressOutcome,
    after_ms: u64,
) -> LabError {
    error.message = format!(
        "{}; the press of step {} was performed and recorded and the step is closed: take the \
         screen after it with capture --record (MCP: ac_lab_observe with capture and record), do \
         not press again",
        error.message, outcome.click.step
    );
    with_leading_details(
        error,
        [
            ("record", to_json(outcome)),
            ("after", json!({"after_ms": after_ms, "taken": false})),
        ],
    )
}

/// #463 S1-2 fix round 3: the error of a press whose after loop a capture after its first stopped
/// ([`after_loop_stopped`]): its message also saying that the press stands, its details beginning
/// with `record` (the press as recorded, its after frame the last capture taken, `after_capture`
/// with `unfinished`), then `after: {after_ms, taken: true, captures}` and the capture's own
/// members.
fn after_loop_stopped_press(mut error: LabError, outcome: &RecordPressOutcome) -> LabError {
    error.message = format!(
        "{}; the press of step {} was performed and recorded and the step is closed: take a later \
         screen with capture --record (MCP: ac_lab_observe with capture and record) when one is \
         needed, do not press again",
        error.message, outcome.click.step
    );
    with_leading_details(error, [("record", to_json(outcome))])
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
            record_started_at_unix_ms: session.recording.record_started_at_unix_ms,
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
        optional: optional_view(step),
        forbidden: step.forbidden.clone(),
        anchor: step.anchor.clone(),
        inserted_after: step.inserted_after,
    }
}

fn optional_view(step: &RecordingStep) -> Option<StepOptionalView> {
    step.optional.as_ref().map(|optional| StepOptionalView {
        settle_ms: optional.settle_ms,
    })
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
                value_test: mark.value_test.clone(),
                // #463 L2 fix 2: where an agent reads the members a change of names repeats.
                located_text: mark.locates_text().then(|| mark.clone()),
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
            gesture: click.gesture,
            names_changed_after_press: click
                .follow
                .as_ref()
                .is_some_and(|follow| follow.names_changed_after_press),
        }),
        application: step.application.clone(),
        optional: optional_view(step),
        transition: step.transition.as_ref().map(transition_view),
        closed: step.closed,
        closed_by: step.closed_by.clone(),
        forbidden: step.forbidden.clone(),
        anchor: step.anchor.clone(),
        // #463 R1: where an agent reads the select it declares again to change its default.
        select: step.select.clone(),
        inserted_after: step.inserted_after,
    }
}
