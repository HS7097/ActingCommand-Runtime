// SPDX-License-Identifier: AGPL-3.0-only

//! Public entry points of the Lab recording core (Workflow #336 L3). Every state-writing
//! call takes the per-instance `RecordingLock`, so the lock is taken before any write;
//! `record_status` only reads.

use crate::LabOcr;
use crate::recording::archive::ARCHIVED;
use crate::recording::container::{
    container_kind, encode, extension, file_name, preflight_lab_dir, replace_in_place,
    write_lab_dir,
};
use crate::recording::crosscheck::{
    FrameCache, admit, cross_check, evaluator_bundle, first_decision, round_trip,
};
use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, Plan, StopSettings,
    application_steps_summary, optional_steps_summary, plan as generate_plan, render,
    transitions_summary, validate_timeout_option,
};
use crate::recording::marks::{declares_mark, match_metric};
use crate::recording::model::{
    ApplicationPlan, AttachFrameOutcome, AttachFrameRequest, ClickEffect, ClickPlan, ClickView,
    CommitApplicationOutcome, CommitApplicationRequest, CommitClickOutcome, CommitClickRequest,
    FrameView, LAB_RECORD_MARK_SCHEMA, LAB_RECORDING_MAX_NEXT_FRAME_MS,
    LAB_RECORDING_MAX_SETTLE_MS, LabRecording, LabRecordingStart, LabStatus, LabStatusView,
    MarkOutcome, MarkRequest, MarkStatusView, MarkView, NextFrameOutcome, OpaqueJson,
    PlanApplicationRequest, PlanClickRequest, RecordStartOptions, RecordStepOutcome,
    RecordStepRequest, RecordStopOptions, RecordStopOutcome, RecordedFrame, RecordedMark,
    RecordingArtifact, RecordingDefaults, RecordingStep, SelfTestStatus, StepClick,
    StepOptionalView, StepStateView, StepTransition, StepView, TransitionSpec, TransitionView,
};
use crate::recording::steps::{
    ArrivalMode, FrameMeta, Session, application_action, application_with_click, apply_marks,
    apply_step_action, apply_transition, arrive, check_device_arrival, commit_application,
    commit_click, default_recording_defaults, effective_indices, new_recording, not_active,
    open_session, open_session_for_marks, open_step, plan_application, plan_click, save,
    stopped_change, to_json,
};
use crate::recording::store::{
    LabFile, OldRecordView, blocked, create_recording, invalid, lab_dir, load_lab, now_unix_ms,
    page_name_valid, read_old_record, read_verified, recording_path, save_recording, state_io,
    task_id_valid_for_lab, with_details,
};
use crate::recording::{
    RecordingLock,
    frames::{check_frame_size, decode_frame, read_stored_frame},
};
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
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
            let steps = recording
                .steps
                .iter()
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
            Some(artifact) => stop_already_generated(&recording, artifact, options),
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
    let settings = StopSettings {
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
    };
    generate_and_write(state_dir, &old, &recording, &settings, options, ocr, false)
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
    let (cache, crops) = load_package_inputs(&plan)?;

    // First pass: admit the package, decide its first step and cross-check its pages.
    let first_entries = render(&plan, None, &crops)?;
    let first_prepared = admit(&first_entries, ocr)?;
    let bundle = evaluator_bundle(&first_entries, ocr)?;
    let evaluator = bundle.loaded_bundle().evaluator().ok_or_else(|| {
        LabError::safety_blocked(
            "record_artifact_admission_failed",
            "the admitted package has no recognition evaluator",
            &["session_record"],
        )
        .with_details(json!({"stage": "evaluator"}))
    })?;
    // Workflow #396 REC-4: the evaluator gives the box a followed template matches on a frame.
    let decision = first_decision(&first_prepared, &plan, &cache, ocr, evaluator)?;
    let checks = cross_check(&plan, evaluator, &cache, ocr)?;
    let mut warnings = plan.warnings.clone();
    warnings.extend(decision.warning.clone());
    warnings.extend(checks.warnings.clone());
    let mut provenance = Map::new();
    provenance.insert("warnings".to_string(), json!(warnings));
    provenance.insert(
        "arrival_by_time_window".to_string(),
        json!(checks.arrival_by_time_window),
    );
    provenance.insert("cross_check".to_string(), checks.cross_check.clone());

    // Final package: the same content with the self-check results in its provenance, encoded,
    // expanded again and admitted again.
    let entries = render(&plan, Some(&provenance), &crops)?;
    let kind = container_kind(&entries);
    let bytes = encode(&entries, kind)?;
    round_trip(&bytes, kind, &entries)?;
    let prepared = admit(&entries, ocr)?;
    let final_decision = first_decision(&prepared, &plan, &cache, ocr, evaluator)?;
    if final_decision.value != decision.value {
        return Err(LabError::safety_blocked(
            "record_artifact_admission_failed",
            "the first decision of the final package differs from the checked one",
            &["session_record"],
        )
        .with_details(json!({
            "stage": "first_decision",
            "checked": decision.value,
            "final": final_decision.value
        })));
    }

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
    let lab_dir_status = if options.dry_run {
        None
    } else {
        if let Some(parent) = copy_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                state_io(format!("failed to create {}: {error}", parent.display()))
            })?;
        }
        replace_in_place(&copy_path, &bytes, &mut written)?;
        match &lab_dir_plan {
            Some(dir_plan) => Some(written_status(write_lab_dir(
                dir_plan,
                &bytes,
                &mut written,
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
    let pages = plan
        .steps
        .iter()
        .flat_map(|step| step.page.iter().chain(step.transition_page.iter()))
        .cloned()
        .collect::<Vec<_>>();
    lab.insert("pages".to_string(), json!(pages));
    lab.insert("transitions".to_string(), json!(transitions_summary(&plan)));
    lab.insert(
        "optional_steps".to_string(),
        json!(optional_steps_summary(&plan, &checks.same_as)?),
    );
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
    lab.insert(
        "marks_not_evaluated".to_string(),
        json!(
            plan.emitted
                .iter()
                .filter(|id| plan
                    .marks
                    .get(*id)
                    .is_some_and(|mark| { mark.self_test.status == SelfTestStatus::NotEvaluated }))
                .collect::<Vec<_>>()
        ),
    );
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
        "binding_requires".to_string(),
        json!(binding_requires(&plan)),
    );
    Ok(RecordStopOutcome {
        lab_status: Some(status.to_string()),
        lab: Some(OpaqueJson::from_serializable(&Value::Object(lab))?),
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
    if let Some(application) = &request.application {
        application_action(&application.action)?;
        if request.click.is_some() || request.click_guard.is_some() || request.retry.is_some() {
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
    };
    let step = if let Some(action) = &request.step_action {
        let (status, step) = apply_step_action(session, action)?;
        outcome.status = status.to_string();
        step
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
        record_started_at_unix_ms: session.recording.record_started_at_unix_ms,
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
                !request.add.is_empty()
                    && request
                        .add
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
    }
}
