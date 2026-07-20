// SPDX-License-Identifier: AGPL-3.0-only

impl<P: LabPorts> Lab<P> {
    pub fn lab_run(&mut self, request: LabRunRequest) -> CliOutcome<LabRunResponse> {
        run_lab(self, request)
    }

    pub fn lab_validate(
        &mut self,
        request: LabValidateRequest,
    ) -> CliOutcome<LabValidateResponse> {
        validate_lab_package_zip_with_expected(
            &request.zip_path,
            request.expected_input_sha256,
        )
    }
}

fn run_lab<P: LabPorts>(
    lab: &mut Lab<P>,
    mut request: LabRunRequest,
) -> CliOutcome<LabRunResponse> {
    let zip_path = request.zip_path.clone();
    let out_path = request.out_path.clone();
    let ledger_session = lab.ports_mut().ledger().run_session();
    let mut ctx = LabRunContext::create_with_context(
        &request.run_root,
        &zip_path,
        request.process.clone(),
        lab.ports().clock(),
        ledger_session,
    )?;
    let run_dir = ctx.run_dir.clone();
    if path_is_inside_from(
        &out_path,
        &run_dir,
        request.process.current_dir.as_deref(),
    ) {
        return Err(CliError::usage(
            "--out must not be inside the Lab run directory",
        ));
    }
    let run_dir_string = run_dir.display().to_string();
    ctx.set_phase("run_started");
    ctx.event(
        "run_started",
        json!({"input_zip": zip_path, "out": out_path}),
    )?;

    let result = execute_lab_run(&mut ctx, lab.ports(), &mut request);
    match result {
        Ok(run_state) => {
            ctx.finish(&out_path, true, None, Some(&run_state))?;
            let completed = ctx.project_completed_run_from_ledger()?;
            let output_zip = completed.require_output_zip()?;
            let output_zip_path = output_zip.path.clone();
            let output_zip_sha256 = output_zip.sha256.clone();
            Ok(LabRunResponse {
                ok: completed.ok,
                status: completed.status,
                run_id: completed.run_id,
                result_zip: output_zip_path.clone(),
                run_dir: run_dir_string,
                run_dir_cleaned: true,
                out: output_zip_path,
                output_zip_sha256,
                ledger: LabRunLedgerResponse {
                    projection_source: "runtime_global_ledger".to_string(),
                    path: completed.ledger_path.display().to_string(),
                    terminal_receipt: completed.record_type,
                },
                screenshot_count: ctx.screenshots.len(),
                executed_step_count: ctx.steps.len(),
            })
        }
        Err(err) => {
            ctx.set_phase("run_failed");
            let message = err.message.clone();
            let archive = ctx.finish(&out_path, false, Some(&message), None);
            match archive {
                Ok(_) => {
                    let completed = ctx.project_completed_run_from_ledger()?;
                    let output_zip = completed.require_output_zip()?;
                    let mut err = err;
                    err.message = format!(
                        "{}; failure report written to {}",
                        err.message, output_zip.path
                    );
                    Err(err)
                }
                Err(write_err) => Err(CliError::package_invalid(format!(
                    "failed to write Lab-1y output package after error: {}; original error: {}",
                    write_err.message, err.message
                ))),
            }
        }
    }
}

fn validate_lab_package_zip_with_expected(
    zip_path: &Path,
    expected_input_sha256: Option<Sha256Hash>,
) -> CliOutcome<LabValidateResponse> {
    let contained =
        load_lab_package_for_validation(zip_path, "lab-validate", expected_input_sha256)?;
    let input_sha256 = contained.sha256.clone();
    let hash_source = contained.hash_source.to_string();
    let externally_verified = contained.externally_verified;
    let entry_count = contained.bundle.entry_count();
    let control = lab_control_from_bundle(&contained.bundle)?;
    control.validate()?;
    let resources = load_lab_resources_from_bundle(contained.bundle, &control)?;
    Ok(LabValidateResponse {
        zip: zip_path.display().to_string(),
        status: "valid".to_string(),
        input_sha256,
        hash_source,
        externally_verified,
        entry_count,
        control: LabValidateControlResponse {
            package_id: control.package_id,
            execution_mode: control.execution_mode,
            game: control.game,
            server: control.server,
            resolution: LabRunResolution {
                width: control.resolution.width,
                height: control.resolution.height,
            },
            entry_task_id: control.entry_task_id,
        },
        resources: LabValidateResourcesResponse {
            resource_root: resources.resource_root.display().to_string(),
            manifest: resources.manifest_path.display().to_string(),
            operation: resources.operation_path.display().to_string(),
            operation_count: resources.operation_bundle.operations.len(),
            pack: resources.pack_path.display().to_string(),
            recognition_unsupported_target_count: resources.evaluator.unsupported_target_count(),
            recognition_unsupported_targets: resources
                .evaluator
                .unsupported_targets()
                .iter()
                .map(|target| LabUnsupportedTargetResponse {
                    id: target.id.clone(),
                    reason: target.reason.clone(),
                })
                .collect(),
            pages: resources.pages_path.display().to_string(),
            navigation: resources
                .navigation_path
                .as_ref()
                .map(|path| path.display().to_string()),
        },
    })
}

/// Admits and prepares the exact package bytes for both validation reporting and execution.
pub fn prepare_lab_package_bytes(
    input_label: &str,
    bytes: &[u8],
    expected_input_sha256: ExternalExpectedSha256,
) -> CliOutcome<(
    PreparedContainedTask,
    LabContainedPackageValidationResponse,
)> {
    let admitted = ExternallyVerifiedBundle::load(input_label, bytes, expected_input_sha256)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let prepared = PreparedContainedTask::from_verified_bundle(&admitted)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let sha256 = admitted.loaded_bundle().verified_hash().to_string();
    let task_count = admitted.loaded_bundle().task_count();
    let entries = admitted
        .loaded_bundle()
        .entry_paths()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let bundle = admitted.into_loaded_bundle();
    let entry_count = bundle.entry_count();
    let control = lab_control_from_bundle(&bundle)?;
    control.validate()?;
    let resources = load_lab_resources_from_bundle(bundle, &control)?;
    let validation = LabValidateResponse {
        zip: input_label.to_string(),
        status: "valid".to_string(),
        input_sha256: sha256,
        hash_source: "externally_supplied".to_string(),
        externally_verified: true,
        entry_count,
        control: LabValidateControlResponse {
            package_id: control.package_id,
            execution_mode: control.execution_mode,
            game: control.game,
            server: control.server,
            resolution: LabRunResolution {
                width: control.resolution.width,
                height: control.resolution.height,
            },
            entry_task_id: control.entry_task_id,
        },
        resources: LabValidateResourcesResponse {
            resource_root: resources.resource_root.display().to_string(),
            manifest: resources.manifest_path.display().to_string(),
            operation: resources.operation_path.display().to_string(),
            operation_count: resources.operation_bundle.operations.len(),
            pack: resources.pack_path.display().to_string(),
            recognition_unsupported_target_count: resources.evaluator.unsupported_target_count(),
            recognition_unsupported_targets: resources
                .evaluator
                .unsupported_targets()
                .iter()
                .map(|target| LabUnsupportedTargetResponse {
                    id: target.id.clone(),
                    reason: target.reason.clone(),
                })
                .collect(),
            pages: resources.pages_path.display().to_string(),
            navigation: resources
                .navigation_path
                .as_ref()
                .map(|path| path.display().to_string()),
        },
    };
    Ok((
        prepared,
        LabContainedPackageValidationResponse {
            validation,
            task_count,
            entries,
        },
    ))
}

/// Deep-validates package bytes through the same preparation path used for execution.
pub fn validate_lab_package_bytes(
    input_label: &str,
    bytes: &[u8],
    expected_input_sha256: ExternalExpectedSha256,
) -> CliOutcome<LabContainedPackageValidationResponse> {
    prepare_lab_package_bytes(input_label, bytes, expected_input_sha256)
        .map(|(_prepared, validation)| validation)
}

#[cfg(test)]
fn validate_lab_package_zip(zip_path: &Path) -> CliOutcome<LabValidateResponse> {
    validate_lab_package_zip_with_expected(zip_path, None)
}

fn execute_lab_run<P: LabPorts>(
    ctx: &mut LabRunContext<'_, P::Ledger>,
    ports: &P,
    request: &mut LabRunRequest,
) -> CliOutcome<RunState> {
    ctx.set_phase("input_unpacked");
    let contained = load_lab_package_for_run(
        &request.zip_path,
        "lab-run",
        request.expected_input_sha256,
    )?;
    ctx.input_zip_sha256 = Some(contained.sha256.clone());
    ctx.input_entries = contained.bundle.entry_paths().map(str::to_string).collect();
    ctx.event(
        "input_unpacked",
        json!({"entry_count": ctx.input_entries.len(), "containment": "memory", "input_sha256": contained.sha256}),
    )?;

    ctx.set_phase("control_loaded");
    let control = lab_control_from_bundle(&contained.bundle)?;
    control.validate()?;
    ctx.control = Some(control.clone());
    let mut frame_store_config =
        FrameStoreConfig::default().with_memory_source(request.process.memory_source);
    control.frame_store.apply_to(&mut frame_store_config);
    request
        .frame_store_override
        .apply_to(&mut frame_store_config);
    ctx.set_frame_store_config(frame_store_config)?;
    if control.producer.is_none() {
        ctx.event(
            "producer_missing",
            json!({"severity": "warning", "message": "control producer is missing; provenance is incomplete but not blocking"}),
        )?;
    }
    ctx.event(
        "control_loaded",
        json!({
            "package_id": control.package_id,
            "game": control.game,
            "server": control.server,
            "entry_task_id": control.entry_task_id,
            "producer_present": control.producer.is_some(),
            "trusted_execution_present": control.trusted_execution.is_some()
        }),
    )?;

    ctx.requested_capture_interval_ms = request.capture_interval_override.unwrap_or(
        control
            .capture_interval_ms
            .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS),
    );
    let timeout_ms = control.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let step_timeout_ms = control.step_timeout_ms.unwrap_or(DEFAULT_STEP_TIMEOUT_MS);
    let max_steps = control.max_steps.unwrap_or(DEFAULT_MAX_STEPS);

    ctx.set_phase("resources_loaded");
    let resources = load_lab_resources_from_bundle(contained.bundle, &control)?;
    ctx.event(
        "resources_loaded",
        json!({
            "manifest": resources.manifest_path,
            "operation": resources.operation_path,
            "pack": resources.pack_path,
            "pages": resources.pages_path,
            "navigation": resources.navigation_path,
            "operation_goal": resources.operation_bundle.goal,
            "entry_page": resources.operation_bundle.entry_page,
            "target_page": resources.operation_bundle.target_page,
            "operation_defaults": resources.operation_bundle.defaults.to_json()
        }),
    )?;

    let app_config = ports.config().load()?;
    let selected_id = select_device_id(request, &control, &app_config)?;
    let device = request.device_resolver.resolve_selected(&selected_id)?;
    ctx.instance = Some(device.serial().to_string());
    ctx.adb_path = Some(device.adb_provenance().to_string());
    ctx.ensure_ledger()?;

    ctx.set_phase("runtime_effects_bound");

    let requested_capture_backend = request
        .capture_backend_override
        .or(control.capture_backend_choice()?)
        .unwrap_or_default();
    let capture_observation = CaptureBackendObservation::default();
    let mut capture = ports.capture_factory().open(CaptureBackendRequest {
        instance_alias: Some(selected_id.clone()),
        config: device
            .capture_config()
            .clone()
            .with_requested(requested_capture_backend),
        observation: Some(capture_observation.clone()),
    })?;
    let capture_report = capture_observation.snapshot()?;
    ctx.capture_backend_requested = Some(requested_capture_backend);
    ctx.capture_backend_used = Some(capture_report.used);
    ctx.capture_backend_attempts = capture_report.attempts;
    for attempt in ctx.capture_backend_attempts.clone() {
        ctx.event(
            "capture_backend_attempt",
            capture_backend_attempt_json(&attempt),
        )?;
    }
    ctx.record_capture_backend_selection()?;
    let mut input = None::<Box<dyn InputBackend>>;
    let started = Instant::now();
    let mut state = RunState {
        control,
        resources,
        current_page: None,
        failed_step_id: None,
    };
    let run_operations = state
        .resources
        .operation_bundle
        .operations
        .iter()
        .map(|operation| RunOperationCandidate::new(&operation.id, &operation.from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(run_decision_error)?;
    let run_config = RunStateConfig::new(
        &state.control.game,
        state.resources.operation_bundle.target_page.clone(),
        state.control.stop_on_confirmation.unwrap_or(true),
        state
            .resources
            .operation_bundle
            .max_task_retries
            .unwrap_or(1),
        u32::try_from(max_steps)
            .map_err(|_| CliError::package_invalid("control max_steps exceeds u32"))?,
    )
    .map_err(run_decision_error)?;
    let mut run_machine = RunStateMachine::new(run_config, 0).map_err(run_decision_error)?;
    let actionable_page_candidates = if state.control.execution_mode == "recognize_only" {
        None
    } else {
        Some(actionable_page_ids(&state.resources, &state.control)?)
    };
    let initial_page_candidates = if state.control.execution_mode == "recognize_only" {
        None
    } else {
        Some(initial_page_ids(&state.resources, &state.control)?)
    };

    let first = capture_until_matched_page(
        ctx,
        capture.as_mut(),
        &state.resources,
        "initial",
        step_timeout_ms,
        &state.control,
        initial_page_candidates.as_deref(),
    )?;
    state.current_page = first.matched_anchor(&state.control.game);
    run_machine
        .observe_page(state.current_page.clone())
        .map_err(run_decision_error)?;

    if state.control.execution_mode == "recognize_only" {
        ctx.event(
            "recognize_only_finished",
            json!({"matched_page": first.matched_page, "matched_anchor": state.current_page}),
        )?;
        return Ok(state);
    }

    let terminal_error = loop {
        if started.elapsed() > Duration::from_millis(timeout_ms) {
            return Err(CliError::device(format!(
                "Lab-1y run timeout after {timeout_ms}ms"
            )));
        }
        match run_machine
            .next_directive(&run_operations)
            .map_err(run_decision_error)?
        {
            RunDirective::AwaitPage => {
                let scene = capture_until_matched_page(
                    ctx,
                    capture.as_mut(),
                    &state.resources,
                    "page_wait",
                    step_timeout_ms,
                    &state.control,
                    actionable_page_candidates.as_deref(),
                )?;
                let current_page = scene.matched_anchor(&state.control.game).ok_or_else(|| {
                    CliError::device("no page matched before operation selection")
                })?;
                state.current_page = Some(current_page.clone());
                run_machine
                    .observe_page(Some(current_page))
                    .map_err(run_decision_error)?;
            }
            RunDirective::ExecuteOperation {
                operation_id,
                current_page,
                step_index,
            } => {
                let operation = state
                    .resources
                    .operation_bundle
                    .operations
                    .iter()
                    .find(|operation| operation.id == operation_id)
                    .ok_or_else(|| {
                        CliError::device(format!(
                            "execution-kernel selected unknown operation '{operation_id}'"
                        ))
                    })?
                    .clone();
                match execute_operation_with_retries(
                    ctx,
                    capture.as_mut(),
                    &mut input,
                    OperationExecutionRequest {
                        device: DeviceInputRequest {
                            instance_alias: &selected_id,
                            factory: ports.input_factory(),
                            config: device.touch_config(),
                        },
                        resources: &state.resources,
                        bundle: &state.resources.operation_bundle,
                        control: &state.control,
                        operation: &operation,
                        current_page: &current_page,
                        step_index: usize::try_from(step_index).map_err(|_| {
                            CliError::device("execution-kernel step index exceeds usize")
                        })?,
                        step_timeout_ms,
                        candidate_pages: actionable_page_candidates.as_deref(),
                    },
                )? {
                    OperationRunOutcome::Success { current_page } => {
                        state.current_page = current_page.clone();
                        run_machine
                            .operation_succeeded(&operation_id, current_page)
                            .map_err(run_decision_error)?;
                    }
                    OperationRunOutcome::NeedsRecovery(trigger) => {
                        let terminal = run_machine
                            .operation_needs_recovery(trigger)
                            .map_err(run_decision_error)?;
                        let RunDirective::Terminal(terminal) = terminal else {
                            return Err(CliError::device(
                                "execution-kernel recovery transition did not reach terminal state",
                            ));
                        };
                        break project_run_terminal(ctx, &mut state, &terminal)?;
                    }
                }
            }
            RunDirective::Continue { .. } => {
                return Err(CliError::device(
                    "execution-kernel returned internal continue directive from next_directive",
                ));
            }
            RunDirective::Terminal(terminal) => {
                break project_run_terminal(ctx, &mut state, &terminal)?;
            }
        }
    };

    if let Some(mut backend) = input {
        combine_operation_and_close(Ok(()), backend.close())
            .map_err(|err| CliError::device(err.to_string()))?;
    }
    if let Some(error) = terminal_error {
        return Err(error);
    }
    Ok(state)
}

fn project_run_terminal<L: LedgerSink>(
    ctx: &mut LabRunContext<'_, L>,
    state: &mut RunState,
    terminal: &RunTerminal,
) -> CliOutcome<Option<CliError>> {
    let details = serde_json::to_value(terminal).map_err(|error| {
        CliError::device(format!(
            "failed to serialize execution-kernel run terminal: {error}"
        ))
    })?;
    ctx.event("run_terminal_decided", details.clone())?;
    match terminal {
        RunTerminal::Completed { current_page } => {
            state.current_page = current_page.clone();
            Ok(None)
        }
        RunTerminal::SuccessorSuggested { suggestion } => {
            state.failed_step_id = Some(suggestion.source_operation_id.clone());
            ctx.event("successor_suggested", json!({"suggestion": suggestion}))?;
            Ok(Some(
                CliError::safety_blocked(
                    "successor_suggested",
                    format!(
                        "operation '{}' requested successor task '{}'; scheduler decision is required",
                        suggestion.source_operation_id, suggestion.task_id
                    ),
                    &["scheduler_decision"],
                )
                .with_details(details),
            ))
        }
        RunTerminal::PausedNeedsHuman { pause } => {
            state.failed_step_id = pause.operation_id.clone();
            ctx.event("paused_needs_human", json!({"pause": pause}))?;
            Ok(Some(
                CliError::safety_blocked(
                    "paused_needs_human",
                    "run state requires human attention before execution can continue",
                    &["human_attention"],
                )
                .with_details(details),
            ))
        }
    }
}

fn select_device_id(
    request: &LabRunRequest,
    control: &LabControl,
    config: &crate::UserConfig,
) -> CliOutcome<String> {
    let selected_id = match &request.instance {
        Some(instance) => Some(instance.clone()),
        None => {
            let game = request.game.as_ref().unwrap_or(&control.game);
            let server = request.server.as_ref().unwrap_or(&control.server);
            config.instances.iter().find_map(|(id, instance)| {
                (instance.game.as_ref() == Some(game) && instance.server.as_ref() == Some(server))
                    .then_some(id.clone())
            })
        }
    };
    selected_id.ok_or_else(|| {
        CliError::instance(
            "could not resolve instance; pass --instance or configure instance.<id>.game/server",
        )
    })
}

struct ContainedLabInput {
    sha256: String,
    hash_source: &'static str,
    externally_verified: bool,
    bundle: LoadedBundle,
}

fn load_lab_package_for_validation(
    zip_path: &Path,
    instance_label: &str,
    expected_input_sha256: Option<Sha256Hash>,
) -> CliOutcome<ContainedLabInput> {
    let bytes = open_published_package(zip_path)?.read_all()?;
    let externally_verified = expected_input_sha256.is_some();
    let expected = expected_input_sha256.unwrap_or_else(|| Sha256Hash::digest(&bytes));
    let instance = InstanceId::new(instance_label).map_err(containment_error)?;
    let mut containment = Containment::new();
    containment
        .load(&instance, &bytes, &expected)
        .map_err(containment_error)?;
    let bundle = containment
        .take_loaded(&instance)
        .ok_or_else(|| CliError::package_invalid("containment did not retain loaded Lab bundle"))?;
    Ok(ContainedLabInput {
        sha256: expected.to_string(),
        hash_source: if externally_verified {
            "externally_supplied"
        } else {
            "self_computed_provenance_only"
        },
        externally_verified,
        bundle,
    })
}

fn load_lab_package_for_run(
    zip_path: &Path,
    instance_label: &str,
    expected_input_sha256: ExternalExpectedSha256,
) -> CliOutcome<ContainedLabInput> {
    let bytes = open_published_package(zip_path)?.read_all()?;
    let admitted = ExternallyVerifiedBundle::load(instance_label, &bytes, expected_input_sha256)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let sha256 = admitted.loaded_bundle().verified_hash().to_string();
    Ok(ContainedLabInput {
        sha256,
        hash_source: "externally_supplied",
        externally_verified: true,
        bundle: admitted.into_loaded_bundle(),
    })
}

fn containment_error(err: ContainmentError) -> CliError {
    CliError::package_invalid(err.to_string())
}
