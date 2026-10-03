// SPDX-License-Identifier: AGPL-3.0-only

//! Public entry points of the Lab recording core (Workflow #336 L3). Every state-writing
//! call takes the per-instance `RecordingLock`, so the lock is taken before any write;
//! `record_status` only reads.

use crate::recording::container::{
    container_kind, encode, extension, preflight_lab_dir, table_digest, write_lab_dir,
    write_recording_copy,
};
use crate::recording::crosscheck::{
    FrameCache, admit, cross_check, evaluator_bundle, first_decision, round_trip,
};
use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, Plan, StopSettings,
    application_steps_summary, optional_steps_summary, plan as generate_plan, render,
    transitions_summary, validate_timeout_option,
};
use crate::recording::marks::match_metric;
use crate::recording::model::{
    ApplicationPlan, AttachFrameOutcome, AttachFrameRequest, ClickEffect, ClickPlan, ClickView,
    CommitApplicationOutcome, CommitApplicationRequest, CommitClickOutcome, CommitClickRequest,
    FrameView, LAB_RECORD_MARK_SCHEMA, LAB_RECORDING_MAX_SETTLE_MS, LabRecording,
    LabRecordingStart, LabStatus, LabStatusView, MarkOutcome, MarkRequest, MarkStatusView,
    MarkView, OpaqueJson, PlanApplicationRequest, PlanClickRequest, RecordStartOptions,
    RecordStopOptions, RecordStopOutcome, RecordedFrame, RecordingArtifact, RecordingDefaults,
    RecordingStep, SelfTestStatus, StepClick, StepOptionalView, StepStateView, StepTransition,
    StepView, TransitionSpec, TransitionView,
};
use crate::recording::steps::{
    ArrivalMode, FrameMeta, application_action, application_with_click, apply_marks,
    apply_step_action, apply_transition, arrive, check_device_arrival, commit_application,
    commit_click, default_recording_defaults, effective_indices, new_recording, not_active,
    open_session, open_step, plan_application, plan_click, save, to_json,
};
use crate::recording::store::{
    LabFile, blocked, create_recording, hex_sha256, invalid, lab_dir, load_lab, now_unix_ms,
    page_name_valid, read_old_record, read_verified, recording_path, save_recording, state_io,
    task_id_valid_for_lab, with_details,
};
use crate::recording::{
    RecordingLock,
    frames::{check_frame_size, decode_frame, read_stored_frame},
};
use actingcommand_contract::{CONTENT_DIRECTORY_V1, LabError, LabErrorClass, LabResult};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
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

/// `record stop` before the session file is stopped (Workflow #336 L4, frozen model section
/// 4): a Lab recording with steps generates its `linear_steps` package, runs every self-check
/// and writes the package (section 4.7) and the stopped recording; a stopped recording with a
/// package copies it to `--lab-dir` again (`already_generated`). `dry_run` runs everything and
/// writes nothing. Any refusal leaves the recording and the session active. Without Lab steps
/// the outcome has no `lab` and `record stop` behaves as before.
pub fn record_stop(
    lock: &RecordingLock,
    options: &RecordStopOptions,
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
    let recording = match load_lab(state_dir, &old)? {
        LabFile::Ready(recording) => *recording,
        LabFile::Missing | LabFile::Unavailable(_) => return Ok(none),
    };
    if recording.status == "stopped" {
        return match &recording.artifact {
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
    let plan = generate_plan(&recording, &settings)?;
    let (cache, crops) = load_package_inputs(&plan)?;

    // First pass: admit the package, decide its first step and cross-check its pages.
    let first_entries = render(&plan, None, &crops)?;
    let first_digest = table_digest(&first_entries);
    let first_prepared = admit(&first_entries, &first_digest)?;
    let decision = first_decision(&first_prepared, &plan, &cache)?;
    let bundle = evaluator_bundle(&first_entries, &first_digest)?;
    let evaluator = bundle.loaded_bundle().evaluator().ok_or_else(|| {
        LabError::safety_blocked(
            "record_artifact_admission_failed",
            "the admitted package has no recognition evaluator",
            &["session_record"],
        )
        .with_details(json!({"stage": "evaluator"}))
    })?;
    let checks = cross_check(&plan, evaluator, &cache)?;
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
    let digest = table_digest(&entries);
    let kind = container_kind(&entries);
    let bytes = encode(&entries, kind)?;
    round_trip(&bytes, kind, &entries, &digest)?;
    let prepared = admit(&entries, &digest)?;
    let final_decision = first_decision(&prepared, &plan, &cache)?;
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
    let file_name = format!("{digest}.{ext}");
    let copy_path = lab_dir(state_dir, &recording.record_id)
        .join("out")
        .join(&file_name);
    let lab_dir_plan = options
        .lab_dir
        .as_deref()
        .map(|dir| preflight_lab_dir(Path::new(dir), &file_name, &digest))
        .transpose()?;
    let sha256 = hex_sha256(&bytes);
    let mut written = Vec::new();
    let lab_dir_status = if options.dry_run {
        None
    } else {
        if write_recording_copy(&copy_path, &bytes, &digest)? {
            written.push(copy_path.display().to_string());
        }
        match &lab_dir_plan {
            Some(dir_plan) if dir_plan.present => Some("present"),
            Some(dir_plan) => {
                write_lab_dir(dir_plan, &bytes, &digest, &mut written)?;
                Some("written")
            }
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
        stopped.artifact = Some(RecordingArtifact {
            container: ext.to_string(),
            digest: digest.clone(),
            path: absolute_display(&copy_path),
            lab_dir_path: lab_dir_plan
                .as_ref()
                .map(|dir_plan| absolute_display(&dir_plan.target)),
            sha256: sha256.clone(),
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
    } else {
        "generated"
    };
    let mut lab = Map::new();
    lab.insert("status".to_string(), json!(status));
    lab.insert("dry_run".to_string(), json!(options.dry_run));
    if old.status != "active" {
        lab.insert("session_already_stopped".to_string(), json!(true));
    }
    lab.insert("container".to_string(), json!(ext));
    lab.insert("digest".to_string(), json!(digest));
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
            json!(lab_dir_plan.as_ref().map(|dir_plan| if dir_plan.present {
                "present"
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
        if let Some(dir_plan) = lab_dir_plan.as_ref().filter(|dir_plan| !dir_plan.present) {
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
    lab.insert("sha256".to_string(), json!(sha256));
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
        &digest,
        &plan.package_id,
        plan.requires.as_deref(),
        &package_path,
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

/// The absolute form of a path for the output and the binding snippets.
fn absolute_display(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// `package_ref`, `binding_example`, `task_run_example`, `prerequisite_entry_example` and
/// `catalog_on_failure_example` (section 4.8 with a15, a20 and R22).
fn insert_binding(
    lab: &mut Map<String, Value>,
    digest: &str,
    package_id: &str,
    requires: Option<&str>,
    package_path: &str,
) {
    let reference = json!({"schema_version": CONTENT_DIRECTORY_V1, "sha256": digest});
    lab.insert("package_ref".to_string(), reference.clone());
    lab.insert("requires".to_string(), json!(requires));
    lab.insert(
        "prerequisite_entry_example".to_string(),
        json!({
            "package_id": package_id,
            "package_path": package_path,
            "package_digest": reference
        }),
    );
    lab.insert(
        "binding_example".to_string(),
        json!({
            "procedure_ref": package_id,
            "package_digest": reference,
            "operation_id": "operation.contained_task",
            "yield_points": [],
            "scheduled_execution": {"mode": "device_registry", "package_path": package_path}
        }),
    );
    lab.insert(
        "task_run_example".to_string(),
        json!(format!(
            "actingctl task-run --state-root <root> --instance <alias> --package \"{package_path}\" \
             --package-ref '{{\"schema_version\":\"{CONTENT_DIRECTORY_V1}\",\"sha256\":\"{digest}\"}}'"
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
            "the actingd configuration prerequisite_packages has an entry with package_id \
             {requires} (contracts/linear-steps.md, Prerequisite packages)"
        ));
        items.push(
            "the last package of a prerequisite chain is best a page-graph package: it succeeds \
             with 0 steps when the screen is already on its target page"
                .to_string(),
        );
    } else if !entry {
        items.push(
            "This package declares no prerequisite package. When the screen is not on step 1, \
             the Runtime first runs the return-home package configured in return_home_packages \
             for this game and server, then waits for step 1 (R16), so step 1 should be the \
             final screen of that return-home package (the main interface). A package recorded \
             from another screen declares a prerequisite package that brings the screen to \
             step 1 with --requires. Without a configured return-home package the run fails \
             with contained_task_linear_entry_unmatched."
                .to_string(),
        );
    }
    items.push(
        "The on_failure of the catalog task is best catalog_on_failure_example: a failure of \
         this package's own steps is run once more, and the same problem again suspends the \
         task (R17). The rerun happens immediately (R22): at the first policy evaluation after \
         retry_backoff_ms (0 in the example), without waiting for the next trigger or for \
         cooldown_ms. Changing on_failure changes the catalog, which needs a new approval."
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
             of the stall recovery ladder): expand it into <install root>\\packages\\<game>\\<D>\\ \
             and configure {package, expected_sha256: \"<D>\"}. As a prerequisite or return-home \
             package it restarts the game whenever the single-frame check of the layer above \
             fails."
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
    let bytes = read_verified(Path::new(&artifact.path), &artifact.sha256)?;
    let file_name = format!("{}.{}", artifact.digest, artifact.container);
    let lab_dir_plan = options
        .lab_dir
        .as_deref()
        .map(|dir| preflight_lab_dir(Path::new(dir), &file_name, &artifact.digest))
        .transpose()?;
    let mut written = Vec::new();
    let lab_dir_status = match &lab_dir_plan {
        None => None,
        Some(dir_plan) if dir_plan.present => Some("present"),
        Some(_) if options.dry_run => Some("to_write"),
        Some(dir_plan) => {
            write_lab_dir(dir_plan, &bytes, &artifact.digest, &mut written)?;
            Some("written")
        }
    };
    let package_path = lab_dir_plan.as_ref().map_or_else(
        || artifact.path.clone(),
        |dir_plan| absolute_display(&dir_plan.target),
    );
    let mut lab = Map::new();
    lab.insert("status".to_string(), json!("already_generated"));
    lab.insert("dry_run".to_string(), json!(options.dry_run));
    lab.insert("container".to_string(), json!(artifact.container));
    lab.insert("digest".to_string(), json!(artifact.digest));
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
    lab.insert("sha256".to_string(), json!(artifact.sha256));
    lab.insert("byte_count".to_string(), json!(artifact.byte_count));
    lab.insert(
        "generated_at_unix_ms".to_string(),
        json!(artifact.generated_at_unix_ms),
    );
    insert_binding(
        &mut lab,
        &artifact.digest,
        &artifact.package_id,
        artifact.requires.as_deref(),
        &package_path,
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
        optional: optional_view(step),
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
        optional: optional_view(step),
        transition: step.transition.as_ref().map(transition_view),
        closed: step.closed,
        closed_by: step.closed_by.clone(),
    }
}
