// SPDX-License-Identifier: AGPL-3.0-only

impl<P: LabPorts> Lab<P> {
    pub fn lab_validate(&mut self, request: LabValidateRequest) -> CliOutcome<LabValidateResponse> {
        validate_lab_package_zip(&request.zip_path)
    }
}

/// Workflow #336 PKG-S4 (option 丙): admits the Lab package structurally; no digest of it is
/// taken, computed or reported.
fn validate_lab_package_zip(zip_path: &Path) -> CliOutcome<LabValidateResponse> {
    let contained = load_lab_package_for_validation(zip_path, "lab-validate")?;
    let entry_count = contained.bundle.entry_count();
    let control = lab_control_from_bundle(&contained.bundle)?;
    control.validate()?;
    let resources = load_lab_resources_from_bundle(contained.bundle, &control)?;
    Ok(LabValidateResponse {
        zip: zip_path.display().to_string(),
        status: "valid".to_string(),
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
                    reason: target.to_string(),
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

/// Deep-validates the exact package bytes that will be passed to the execution kernel.
/// Workflow #336 PKG-S4 (option 丙): admission is structural; no digest is taken or reported.
pub fn validate_lab_package_bytes(
    input_label: &str,
    bytes: &[u8],
) -> CliOutcome<LabContainedPackageValidationResponse> {
    let admitted = ExternallyVerifiedBundle::load(input_label, bytes)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    validate_admitted_lab_package(admitted, input_label, || {
        actingcommand_execution_kernel::PreparedContainedTask::load(input_label, bytes).map(drop)
    })
}

/// Workflow #336 PKG-S4 (forced compromise T-01): `validate_lab_package_bytes` for the package
/// `locator` names, read as its path chooses (PKG-S1): a content directory, a `.json` or
/// `.zip` container, or an assembled ZIP. Admission is structural; no digest is taken.
pub fn validate_lab_package_path(
    input_label: &str,
    locator: &Path,
    deadline: std::time::Instant,
) -> CliOutcome<LabContainedPackageValidationResponse> {
    let admitted = ExternallyVerifiedBundle::load_path(input_label, locator, false, None, deadline)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    validate_admitted_lab_package(admitted, input_label, || {
        actingcommand_execution_kernel::PreparedContainedTask::load_path(
            input_label,
            locator,
            None,
            deadline,
        )
        .map(drop)
    })
}

/// The deep validation of an admitted Lab package; `runtime_admission` admits the same package
/// again as the execution kernel does, for the contracts that use Runtime admission.
fn validate_admitted_lab_package(
    admitted: ExternallyVerifiedBundle,
    input_label: &str,
    runtime_admission: impl FnOnce() -> Result<(), actingcommand_execution_kernel::ContainedTaskError>,
) -> CliOutcome<LabContainedPackageValidationResponse> {
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
    let resources = if matches!(
        bundle.operation()["schema_version"].as_str(),
        Some("0.8" | "0.9")
    ) {
        // These contracts use Runtime admission for offline validation.
        runtime_admission().map_err(|error| CliError::package_invalid(error.to_string()))?;
        let evaluator = bundle.evaluator().ok_or_else(|| {
            CliError::package_invalid("missing recognition evaluator for Lab package")
        })?;
        let operation_count = bundle.operation()["operations"]
            .as_array()
            .ok_or_else(|| CliError::package_invalid("missing operations for Lab package"))?
            .len();
        LabValidateResourcesResponse {
            resource_root: bundle.resource_root().to_owned(),
            manifest: bundle.manifest_path().to_owned(),
            operation: bundle.operation_path().to_owned(),
            operation_count,
            pack: bundle
                .recognition_pack_path()
                .ok_or_else(|| {
                    CliError::package_invalid("missing recognition pack for Lab package")
                })?
                .to_owned(),
            recognition_unsupported_target_count: evaluator.unsupported_target_count(),
            recognition_unsupported_targets: evaluator
                .unsupported_targets()
                .iter()
                .map(|target| LabUnsupportedTargetResponse {
                    id: target.id.clone(),
                    reason: target.to_string(),
                })
                .collect(),
            pages: bundle
                .pages_path()
                .ok_or_else(|| CliError::package_invalid("missing page set for Lab package"))?
                .to_owned(),
            navigation: bundle.navigation_path().map(str::to_owned),
        }
    } else {
        let resources = load_lab_resources_from_bundle(bundle, &control)?;
        LabValidateResourcesResponse {
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
                    reason: target.to_string(),
                })
                .collect(),
            pages: resources.pages_path.display().to_string(),
            navigation: resources
                .navigation_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    };
    let validation = LabValidateResponse {
        zip: input_label.to_string(),
        status: "valid".to_string(),
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
        resources,
    };
    Ok(LabContainedPackageValidationResponse {
        validation,
        task_count,
        entries,
    })
}

struct ContainedLabInput {
    bundle: LoadedBundle,
}

/// Workflow #336 PKG-S4 (option 丙): the published Lab package admitted structurally; no
/// digest of its bytes is computed.
fn load_lab_package_for_validation(
    zip_path: &Path,
    instance_label: &str,
) -> CliOutcome<ContainedLabInput> {
    let bytes = open_published_package(zip_path)?.read_all()?;
    let instance = InstanceId::new(instance_label).map_err(containment_error)?;
    let mut containment = Containment::new();
    containment
        .load(&instance, &bytes)
        .map_err(containment_error)?;
    let bundle = containment
        .take_loaded(&instance)
        .ok_or_else(|| CliError::package_invalid("containment did not retain loaded Lab bundle"))?;
    Ok(ContainedLabInput { bundle })
}

fn containment_error(err: ContainmentError) -> CliError {
    CliError::package_invalid(err.to_string())
}
