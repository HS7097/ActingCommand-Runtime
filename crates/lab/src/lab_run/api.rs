// SPDX-License-Identifier: AGPL-3.0-only

impl<P: LabPorts> Lab<P> {
    pub fn lab_validate(&mut self, request: LabValidateRequest) -> CliOutcome<LabValidateResponse> {
        validate_lab_package_zip_with_expected(&request.zip_path, request.expected_input_sha256)
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

/// Deep-validates the exact package bytes that will be passed to the execution kernel.
pub fn validate_lab_package_bytes(
    input_label: &str,
    bytes: &[u8],
    expected_input_sha256: ExternalExpectedSha256,
) -> CliOutcome<LabContainedPackageValidationResponse> {
    let admitted = ExternallyVerifiedBundle::load(input_label, bytes, expected_input_sha256)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let sha256 = admitted
        .loaded_bundle()
        .package_ref()
        .legacy_sha256()
        .ok_or_else(|| {
            CliError::package_invalid("ZIP admission requires a legacy package reference")
        })?
        .to_owned();
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
        actingcommand_execution_kernel::PreparedContainedTask::load(
            input_label,
            bytes,
            expected_input_sha256,
        )
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
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
                    reason: target.reason.clone(),
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
                    reason: target.reason.clone(),
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
        resources,
    };
    Ok(LabContainedPackageValidationResponse {
        validation,
        task_count,
        entries,
    })
}

#[cfg(test)]
fn validate_lab_package_zip(zip_path: &Path) -> CliOutcome<LabValidateResponse> {
    validate_lab_package_zip_with_expected(zip_path, None)
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

fn containment_error(err: ContainmentError) -> CliError {
    CliError::package_invalid(err.to_string())
}
