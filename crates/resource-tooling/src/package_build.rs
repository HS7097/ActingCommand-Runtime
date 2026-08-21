// SPDX-License-Identifier: AGPL-3.0-only

use crate::environment::{AuthoringEnvironmentSnapshot, required_environment_keys};
use crate::package_publish::PackagePublicationTransaction;
#[cfg(test)]
use crate::package_publish::open_published_package;
use crate::resource_convert::{
    Bundle, ConvertOutputs, OperationConverter, ResolvedResourceRoot, canonical_game,
};
use crate::{
    LabPackageControlResponse, LabPackageResourcesResponse, LabPackageValidationResponse,
    PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES, PackageBuildCatalogMetadata, PackageBuildCatalogRequest,
    PackageBuildTaskRequest, PackageBuildTaskResponse, PackageFullArchiveRequest,
    PackageResolution, PackageSource, PackageTaskArchiveRequest,
    UnsupportedRecognitionTargetResponse,
};
use actingcommand_contract::{
    LabError as CliError, LabResult as CliOutcome, SchedulingOutcomeDeclaration,
};
use actingcommand_pack_containment::{
    ContainmentError, ContainmentLimits, Sha256Hash, validate_recognition_metadata,
};
use actingcommand_recognition_pack::{AssetResolver, PackRect, RecognitionPackError};
use serde::{Deserialize, Deserializer, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

const DANGEROUS_EXTENSIONS: &[&str] = &[
    "py", "exe", "bat", "cmd", "ps1", "sh", "js", "vbs", "msi", "dll", "scr", "com", "jar",
];
const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;
const DEFAULT_RECOVERY_TASK_ID: &str = "return_home";
const MAX_STABILITY_TERMINATION_STEPS: u32 = 1_000;
const MAX_CAPTURE_PIXEL_BYTES: usize = 4;

fn deserialize_non_null_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Deterministic package inputs prepared before Lab supplies resolved environment facts.
pub struct PreparedPackageBuildTask {
    source: ResolvedRepo,
    repo: PathBuf,
    resource_root: PathBuf,
    resource_layout: String,
    converter: OperationConverter,
    task_id: String,
    task_ids: Vec<String>,
    outputs: ConvertOutputs,
    resolution: (u32, u32),
    package_id: String,
    execution_mode: String,
    stability_termination: Option<StabilityTermination>,
    out: PathBuf,
    dry_run: bool,
    max_buffered_payload_bytes: usize,
}

/// Loads and validates package inputs without reading Lab or live Runtime state.
pub fn prepare_package_build_task(
    request: PackageBuildTaskRequest,
) -> CliOutcome<PreparedPackageBuildTask> {
    let PackageBuildTaskRequest {
        source,
        temporary_root,
        task_id,
        game,
        server,
        locale,
        package_id,
        execution_mode,
        resolution,
        include_recovery,
        out,
        dry_run,
        max_buffered_payload_bytes,
        env: _,
    } = request;
    validate_max_buffered_payload_bytes(max_buffered_payload_bytes)?;
    let source = ResolvedRepo::from_source(source, &temporary_root)?;
    let repo = source.path().to_path_buf();
    let resource_root = resolve_package_resource_root(&repo)?;
    let converter = load_converter(
        game.as_deref(),
        server.as_deref(),
        locale.as_deref(),
        &resource_root.root,
    )?;
    let mut task_ids = vec![task_id.clone()];
    let includes_recovery = include_recovery
        && task_id != "return_home"
        && converter
            .bundles
            .iter()
            .any(|bundle| bundle.task_id == "return_home");
    if includes_recovery {
        task_ids.push("return_home".to_string());
    }
    let outputs = build_task_outputs(&converter, &task_ids, includes_recovery)?;
    let entry_bundle = find_bundle(&converter, &task_id)?;
    let resolution = parse_resolution(resolution, entry_bundle)?;
    let package_id = package_id
        .unwrap_or_else(|| format!("{}.{}.{}", converter.game, converter.server, task_id));
    let execution_mode = execution_mode.unwrap_or_else(|| "navigable_route".to_string());
    validate_execution_mode(&execution_mode)?;
    let stability_termination =
        validate_entry_stability_termination(entry_bundle, &execution_mode, resolution)?;

    Ok(PreparedPackageBuildTask {
        source,
        repo,
        resource_root: resource_root.root,
        resource_layout: resource_root.layout.to_string(),
        converter,
        task_id,
        task_ids,
        outputs,
        resolution,
        package_id,
        execution_mode,
        stability_termination,
        out,
        dry_run,
        max_buffered_payload_bytes,
    })
}

impl PreparedPackageBuildTask {
    pub fn resource_root(&self) -> &Path {
        &self.resource_root
    }

    pub fn game(&self) -> &str {
        &self.converter.game
    }

    pub fn server(&self) -> &str {
        &self.converter.server
    }

    pub fn required_environment_keys(&self) -> CliOutcome<Vec<String>> {
        let mut keys = generated_output_environment_keys(&self.outputs)?;
        keys.extend(selected_operation_environment_keys(
            &self.converter,
            &self.task_ids,
        )?);
        Ok(keys.into_iter().collect())
    }

    pub fn build(
        mut self,
        environment: &AuthoringEnvironmentSnapshot,
    ) -> CliOutcome<PackageBuildTaskResponse> {
        self.build_with_publication(environment, true)
    }

    /// Builds directly at a caller-owned staging path without publishing a logical output.
    pub fn build_staged(
        mut self,
        environment: &AuthoringEnvironmentSnapshot,
    ) -> CliOutcome<PackageBuildTaskResponse> {
        self.build_with_publication(environment, false)
    }

    fn build_with_publication(
        &mut self,
        environment: &AuthoringEnvironmentSnapshot,
        publish: bool,
    ) -> CliOutcome<PackageBuildTaskResponse> {
        apply_environment_to_outputs(environment, &mut self.outputs)?;
        let mut entries =
            PackageEntries::new(&self.resource_root, self.max_buffered_payload_bytes)?;
        entries.add_json(
            "control.json",
            control_json(
                &self.package_id,
                &self.execution_mode,
                &self.converter.game,
                &self.converter.server,
                self.resolution,
                &self.task_id,
                self.stability_termination.as_ref(),
            )?,
        )?;
        add_resources_json(
            &mut entries,
            &self.resource_root,
            &self.converter,
            &self.task_ids,
            true,
        )?;
        add_selected_operations(
            &mut entries,
            environment,
            &self.resource_root,
            &self.converter,
            &self.task_ids,
        )?;
        add_generated_outputs(&mut entries, &self.converter, &self.outputs)?;
        add_recognition_target_assets(&mut entries, &self.resource_root, &self.outputs.pack)?;
        entries.add_manifest(&self.task_id)?;

        let write = if publish {
            write_and_validate_package(&self.out, entries, self.dry_run)?
        } else {
            write_and_validate_package_staged(&self.out, entries, self.dry_run)?
        };
        let from_remote = self.source.remote_url();
        self.source.cleanup()?;
        Ok(PackageBuildTaskResponse {
            status: if self.dry_run { "validated" } else { "written" }.to_string(),
            mode: "build-task".to_string(),
            repo: self.repo.display().to_string(),
            resource_root: self.resource_root.display().to_string(),
            resource_layout: self.resource_layout.clone(),
            from_remote,
            task_id: self.task_id.clone(),
            included_tasks: self.task_ids.clone(),
            game: self.converter.game.clone(),
            server: self.converter.server.clone(),
            package_id: self.package_id.clone(),
            execution_mode: self.execution_mode.clone(),
            dry_run: self.dry_run,
            out: (!self.dry_run).then(|| self.out.display().to_string()),
            validation: write.validation,
        })
    }
}

pub struct PackageBuildCatalog {
    source: ResolvedRepo,
    repo: PathBuf,
    resource_root: PathBuf,
    resource_layout: String,
    converter: OperationConverter,
    max_buffered_payload_bytes: usize,
}

impl PackageBuildCatalog {
    pub fn open(request: PackageBuildCatalogRequest) -> CliOutcome<Self> {
        validate_max_buffered_payload_bytes(request.max_buffered_payload_bytes)?;
        let source = ResolvedRepo::from_source(request.source, &request.temporary_root)?;
        let repo = source.path().to_path_buf();
        let resource_root = resolve_package_resource_root(&repo)?;
        let converter = load_converter(
            request.game.as_deref(),
            request.server.as_deref(),
            request.locale.as_deref(),
            &resource_root.root,
        )?;
        Ok(Self {
            source,
            repo,
            resource_root: resource_root.root,
            resource_layout: resource_root.layout.to_string(),
            converter,
            max_buffered_payload_bytes: request.max_buffered_payload_bytes,
        })
    }

    pub fn metadata(&self) -> PackageBuildCatalogMetadata {
        PackageBuildCatalogMetadata {
            repo: self.repo.clone(),
            resource_root: self.resource_root.clone(),
            resource_layout: self.resource_layout.clone(),
            from_remote: self.source.remote_url(),
            game: self.converter.game.clone(),
            server: self.converter.server.clone(),
        }
    }

    pub fn task_ids(&self) -> Vec<String> {
        self.converter
            .bundles
            .iter()
            .map(|bundle| bundle.task_id.clone())
            .collect()
    }

    pub fn default_entry_task(&self) -> CliOutcome<String> {
        default_entry_task(&self.converter)
    }

    pub fn resource_root(&self) -> &Path {
        &self.resource_root
    }

    pub fn task_environment_keys(&self, task_id: &str) -> CliOutcome<Vec<String>> {
        find_bundle(&self.converter, task_id)?;
        let task_ids = [task_id.to_string()];
        let outputs = self.converter.build_selected(&task_ids)?;
        let mut keys = generated_output_environment_keys(&outputs)?;
        keys.extend(selected_operation_environment_keys(
            &self.converter,
            &task_ids,
        )?);
        Ok(keys.into_iter().collect())
    }

    pub fn full_environment_keys(&self) -> CliOutcome<Vec<String>> {
        let task_ids = self.task_ids();
        let outputs = self.converter.build_all()?;
        let mut keys = generated_output_environment_keys(&outputs)?;
        keys.extend(selected_operation_environment_keys(
            &self.converter,
            &task_ids,
        )?);
        Ok(keys.into_iter().collect())
    }

    pub fn build_task_archive(
        &self,
        environment: &AuthoringEnvironmentSnapshot,
        request: PackageTaskArchiveRequest,
    ) -> CliOutcome<LabPackageValidationResponse> {
        self.build_task_archive_with_publication(environment, request, true)
    }

    /// Builds directly at a caller-owned staging path for a surrounding group transaction.
    pub fn build_task_archive_staged(
        &self,
        environment: &AuthoringEnvironmentSnapshot,
        request: PackageTaskArchiveRequest,
    ) -> CliOutcome<LabPackageValidationResponse> {
        self.build_task_archive_with_publication(environment, request, false)
    }

    fn build_task_archive_with_publication(
        &self,
        environment: &AuthoringEnvironmentSnapshot,
        request: PackageTaskArchiveRequest,
        publish: bool,
    ) -> CliOutcome<LabPackageValidationResponse> {
        let PackageTaskArchiveRequest {
            task_id,
            package_id,
            execution_mode,
            resolution,
            out,
            dry_run,
            env: _,
        } = request;
        let task_ids = vec![task_id.clone()];
        let mut outputs = self.converter.build_selected(&task_ids)?;
        apply_environment_to_outputs(environment, &mut outputs)?;
        let bundle = find_bundle(&self.converter, &task_id)?;
        let resolution = parse_resolution(resolution, bundle)?;
        validate_execution_mode(&execution_mode)?;
        let stability_termination =
            validate_entry_stability_termination(bundle, &execution_mode, resolution)?;
        let mut entries =
            PackageEntries::new(&self.resource_root, self.max_buffered_payload_bytes)?;
        entries.add_json(
            "control.json",
            control_json(
                &package_id,
                &execution_mode,
                &self.converter.game,
                &self.converter.server,
                resolution,
                &task_id,
                stability_termination.as_ref(),
            )?,
        )?;
        add_resources_json(
            &mut entries,
            &self.resource_root,
            &self.converter,
            &task_ids,
            true,
        )?;
        add_selected_operations(
            &mut entries,
            environment,
            &self.resource_root,
            &self.converter,
            &task_ids,
        )?;
        add_generated_outputs(&mut entries, &self.converter, &outputs)?;
        add_recognition_target_assets(&mut entries, &self.resource_root, &outputs.pack)?;
        entries.add_manifest(&task_id)?;
        if publish {
            Ok(write_and_validate_package(&out, entries, dry_run)?.validation)
        } else {
            Ok(write_and_validate_package_staged(&out, entries, dry_run)?.validation)
        }
    }

    pub fn build_full_archive(
        &self,
        environment: &AuthoringEnvironmentSnapshot,
        request: PackageFullArchiveRequest,
    ) -> CliOutcome<LabPackageValidationResponse> {
        let PackageFullArchiveRequest {
            entry_task_id,
            package_id,
            execution_mode,
            resolution,
            out,
            dry_run,
            env: _,
        } = request;
        let entry_bundle = find_bundle(&self.converter, &entry_task_id)?;
        let resolution = parse_resolution(resolution, entry_bundle)?;
        validate_execution_mode(&execution_mode)?;
        let stability_termination =
            validate_entry_stability_termination(entry_bundle, &execution_mode, resolution)?;
        let mut outputs = self.converter.build_all()?;
        apply_environment_to_outputs(environment, &mut outputs)?;
        let task_ids = self.task_ids();
        let mut entries =
            PackageEntries::new(&self.resource_root, self.max_buffered_payload_bytes)?;
        entries.add_json(
            "control.json",
            control_json(
                &package_id,
                &execution_mode,
                &self.converter.game,
                &self.converter.server,
                resolution,
                &entry_task_id,
                stability_termination.as_ref(),
            )?,
        )?;
        add_resources_json(
            &mut entries,
            &self.resource_root,
            &self.converter,
            &task_ids,
            false,
        )?;
        add_selected_operations(
            &mut entries,
            environment,
            &self.resource_root,
            &self.converter,
            &task_ids,
        )?;
        add_generated_outputs(&mut entries, &self.converter, &outputs)?;
        add_recognition_target_assets(&mut entries, &self.resource_root, &outputs.pack)?;
        entries.add_manifest(&entry_task_id)?;
        Ok(write_and_validate_package(&out, entries, dry_run)?.validation)
    }

    pub fn cleanup(mut self) -> CliOutcome<()> {
        self.source.cleanup()
    }
}

fn build_task_outputs(
    converter: &OperationConverter,
    task_ids: &[String],
    includes_recovery: bool,
) -> CliOutcome<ConvertOutputs> {
    let selected = converter.build_selected(task_ids)?;
    if !includes_recovery {
        return Ok(selected);
    }
    // Recovery may start from pages outside the entry task, so keep the
    // recognition context broad while leaving executable operations selected.
    let full = converter.build_all()?;
    Ok(ConvertOutputs {
        pack: full.pack,
        pages: full.pages,
        navigation: selected.navigation,
        index: selected.index,
        primitives: selected.primitives,
    })
}

fn load_converter(
    game: Option<&str>,
    server: Option<&str>,
    locale: Option<&str>,
    repo: &Path,
) -> CliOutcome<OperationConverter> {
    let game = game.map(canonical_game).transpose()?;
    OperationConverter::load(repo, game.as_deref(), server, locale)
}

fn find_bundle<'a>(converter: &'a OperationConverter, task_id: &str) -> CliOutcome<&'a Bundle> {
    converter
        .bundles
        .iter()
        .find(|bundle| bundle.task_id == task_id)
        .ok_or_else(|| {
            CliError::package_invalid(format!("missing task operations/{task_id}/task.json"))
        })
}

fn default_entry_task(converter: &OperationConverter) -> CliOutcome<String> {
    if converter
        .bundles
        .iter()
        .any(|bundle| bundle.task_id == "return_home")
    {
        Ok("return_home".to_string())
    } else {
        converter
            .bundles
            .first()
            .map(|bundle| bundle.task_id.clone())
            .ok_or_else(|| CliError::package_invalid("package catalog has no operation bundles"))
    }
}

fn parse_resolution(
    resolution: Option<PackageResolution>,
    bundle: &Bundle,
) -> CliOutcome<(u32, u32)> {
    if let Some(resolution) = resolution {
        return Ok((resolution.width, resolution.height));
    }
    let space = bundle
        .data
        .get("coordinate_space")
        .ok_or_else(|| CliError::package_invalid("operation bundle missing coordinate_space"))?;
    let width = json_u32(space, "width")?;
    let height = json_u32(space, "height")?;
    Ok((width, height))
}

fn validate_execution_mode(mode: &str) -> CliOutcome<()> {
    if matches!(mode, "navigable_route" | "recognize_only" | "in_page_guard") {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "unsupported execution mode: {mode}"
        )))
    }
}

fn control_json(
    package_id: &str,
    execution_mode: &str,
    game: &str,
    server: &str,
    resolution: (u32, u32),
    entry_task_id: &str,
    stability_termination: Option<&StabilityTermination>,
) -> CliOutcome<Value> {
    let mut control = ordered_object([
        (
            "schema_version",
            Value::String("Lab-1y.control.v1".to_string()),
        ),
        ("package_id", Value::String(package_id.to_string())),
        ("execution_mode", Value::String(execution_mode.to_string())),
        ("game", Value::String(game.to_string())),
        ("server", Value::String(server.to_string())),
        (
            "resolution",
            ordered_object([
                ("width", Value::from(resolution.0)),
                ("height", Value::from(resolution.1)),
            ]),
        ),
        ("entry_task_id", Value::String(entry_task_id.to_string())),
    ]);
    if let Some(stability_termination) = stability_termination {
        let object = control
            .as_object_mut()
            .expect("control_json always constructs an object");
        object.insert(
            "max_steps".to_string(),
            Value::from(stability_termination.max_steps),
        );
        object.insert(
            "stability_termination".to_string(),
            serde_json::to_value(stability_termination).map_err(|error| {
                CliError::package_invalid(format!(
                    "failed to serialize stability_termination into control.json: {error}"
                ))
            })?,
        );
    }
    Ok(control)
}

fn add_resources_json(
    entries: &mut PackageEntries,
    repo: &Path,
    converter: &OperationConverter,
    task_ids: &[String],
    subset: bool,
) -> CliOutcome<()> {
    let path = repo.join("operations").join("resources.json");
    let mut resources = read_json_value(repo, &path)?;
    if subset {
        let referenced = referenced_resource_ids(converter, task_ids);
        if let Some(array) = resources.get_mut("resources").and_then(Value::as_array_mut) {
            array.retain(|resource| {
                resource
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| referenced.contains(id))
            });
            let resource_count = array.len();
            if let Some(object) = resources.as_object_mut() {
                object.insert("resource_count".to_string(), Value::from(resource_count));
            }
        }
    }
    entries.add_json("resources/operations/resources.json", resources)
}

fn referenced_resource_ids(
    converter: &OperationConverter,
    task_ids: &[String],
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for bundle in &converter.bundles {
        if !task_ids.iter().any(|task_id| task_id == &bundle.task_id) {
            continue;
        }
        for operation in array_field(&bundle.data, "operations") {
            for key in ["consumes", "produces"] {
                for value in operation
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = value.as_str() {
                        ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    ids
}

fn selected_operation_environment_keys(
    converter: &OperationConverter,
    task_ids: &[String],
) -> CliOutcome<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    for bundle in &converter.bundles {
        if task_ids.iter().any(|task_id| task_id == &bundle.task_id) {
            let task = read_json_value(&converter.root, &bundle.dir.join("task.json"))?;
            keys.extend(required_environment_keys(&task)?);
        }
    }
    Ok(keys)
}

fn generated_output_environment_keys(outputs: &ConvertOutputs) -> CliOutcome<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    for output in [
        &outputs.pack,
        &outputs.pages,
        &outputs.navigation,
        &outputs.index,
        &outputs.primitives,
    ] {
        keys.extend(required_environment_keys(output)?);
    }
    Ok(keys)
}

fn apply_environment_to_outputs(
    environment: &AuthoringEnvironmentSnapshot,
    outputs: &mut ConvertOutputs,
) -> CliOutcome<()> {
    for output in [
        &mut outputs.pack,
        &mut outputs.pages,
        &mut outputs.navigation,
        &mut outputs.index,
        &mut outputs.primitives,
    ] {
        environment.apply(output)?;
    }
    Ok(())
}

fn add_selected_operations(
    entries: &mut PackageEntries,
    environment: &AuthoringEnvironmentSnapshot,
    resource_root: &Path,
    converter: &OperationConverter,
    task_ids: &[String],
) -> CliOutcome<()> {
    for bundle in &converter.bundles {
        if task_ids.iter().any(|task_id| task_id == &bundle.task_id) {
            let canonical_task = converter.canonical_task(&bundle.task_id)?;
            entries.add_operation_dir(
                environment,
                &bundle.dir,
                &format!("resources/operations/{}", bundle.task_id),
                resource_root,
                &canonical_task,
            )?;
        }
    }
    Ok(())
}

fn add_generated_outputs(
    entries: &mut PackageEntries,
    converter: &OperationConverter,
    outputs: &ConvertOutputs,
) -> CliOutcome<()> {
    let stem = format!("{}.{}", converter.game, converter.server);
    entries.add_json(
        &format!("resources/recognition/{stem}.pack.json"),
        outputs.pack.clone(),
    )?;
    entries.add_json(
        &format!("resources/recognition/{stem}.pages.json"),
        outputs.pages.clone(),
    )?;
    entries.add_json(
        &format!("resources/navigation/{stem}.navigation.json"),
        outputs.navigation.clone(),
    )?;
    entries.add_json(
        "resources/operations/operations.index.json",
        outputs.index.clone(),
    )?;
    entries.add_json(
        "resources/operations/operations.primitives.json",
        outputs.primitives.clone(),
    )
}

fn add_recognition_target_assets(
    entries: &mut PackageEntries,
    repo: &Path,
    pack: &Value,
) -> CliOutcome<()> {
    for target in pack
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(template_path) = target.get("template_path").and_then(Value::as_str) else {
            continue;
        };
        let zip_path = format!("resources/{template_path}");
        if entries.contains(&zip_path) {
            continue;
        }
        entries.add_file(&repo.join(template_path), &zip_path)?;
    }
    Ok(())
}

enum PackagePayload {
    Buffered {
        bytes: Vec<u8>,
        sha256: String,
    },
    SourceFile {
        source: PathBuf,
        expected_len: u64,
        sha256: String,
    },
}

impl PackagePayload {
    fn sha256(&self) -> &str {
        match self {
            Self::Buffered { sha256, .. } | Self::SourceFile { sha256, .. } => sha256,
        }
    }

    fn expected_len(&self) -> CliOutcome<u64> {
        match self {
            Self::Buffered { bytes, .. } => u64::try_from(bytes.len()).map_err(|_| {
                CliError::package_invalid("buffered package entry length does not fit in u64")
            }),
            Self::SourceFile { expected_len, .. } => Ok(*expected_len),
        }
    }

    fn buffered_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Buffered { bytes, .. } => Some(bytes),
            Self::SourceFile { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackagePayloadStats {
    logical_payload_bytes: u64,
    buffered_payload_bytes: usize,
    peak_buffered_payload_bytes: usize,
    peak_streamed_payload_bytes: usize,
    peak_resident_payload_bytes: usize,
    validation_streamed_payload_bytes: u64,
    peak_validation_streamed_payload_bytes: usize,
    peak_validation_resident_payload_bytes: usize,
}

impl PackagePayloadStats {
    fn add_logical(&mut self, bytes: u64) -> CliOutcome<()> {
        self.logical_payload_bytes =
            self.logical_payload_bytes
                .checked_add(bytes)
                .ok_or_else(|| {
                    CliError::package_invalid("package logical payload byte count overflow")
                })?;
        Ok(())
    }

    fn add_buffered(&mut self, bytes: usize) -> CliOutcome<()> {
        self.buffered_payload_bytes =
            self.buffered_payload_bytes
                .checked_add(bytes)
                .ok_or_else(|| {
                    CliError::package_invalid("package buffered payload byte count overflow")
                })?;
        self.peak_buffered_payload_bytes = self
            .peak_buffered_payload_bytes
            .max(self.buffered_payload_bytes);
        self.peak_resident_payload_bytes = self
            .peak_resident_payload_bytes
            .max(self.buffered_payload_bytes);
        self.add_logical(u64::try_from(bytes).map_err(|_| {
            CliError::package_invalid("package buffered payload does not fit in u64")
        })?)
    }

    fn observe_streamed(&mut self, bytes: usize) -> CliOutcome<()> {
        self.peak_streamed_payload_bytes = self.peak_streamed_payload_bytes.max(bytes);
        let resident = self
            .buffered_payload_bytes
            .checked_add(bytes)
            .ok_or_else(|| {
                CliError::package_invalid("package resident payload byte count overflow")
            })?;
        self.peak_resident_payload_bytes = self.peak_resident_payload_bytes.max(resident);
        Ok(())
    }

    fn observe_validation_streamed(&mut self, bytes: usize) -> CliOutcome<()> {
        self.observe_streamed(bytes)?;
        self.validation_streamed_payload_bytes = self
            .validation_streamed_payload_bytes
            .checked_add(u64::try_from(bytes).map_err(|_| {
                CliError::package_invalid("validation chunk length does not fit in u64")
            })?)
            .ok_or_else(|| {
                CliError::package_invalid("validation streamed payload byte count overflow")
            })?;
        self.peak_validation_streamed_payload_bytes =
            self.peak_validation_streamed_payload_bytes.max(bytes);
        let resident = self
            .buffered_payload_bytes
            .checked_add(bytes)
            .ok_or_else(|| {
                CliError::package_invalid("validation resident payload byte count overflow")
            })?;
        self.peak_validation_resident_payload_bytes =
            self.peak_validation_resident_payload_bytes.max(resident);
        Ok(())
    }
}

struct PackageEntries {
    approved_root: PathBuf,
    files: BTreeMap<String, PackagePayload>,
    max_buffered_payload_bytes: usize,
    stats: PackagePayloadStats,
}

impl PackageEntries {
    fn new(approved_root: &Path, max_buffered_payload_bytes: usize) -> CliOutcome<Self> {
        validate_max_buffered_payload_bytes(max_buffered_payload_bytes)?;
        Ok(Self {
            approved_root: approved_root.to_path_buf(),
            files: BTreeMap::new(),
            max_buffered_payload_bytes,
            stats: PackagePayloadStats::default(),
        })
    }

    fn add_json(&mut self, path: &str, value: Value) -> CliOutcome<()> {
        self.ensure_entry_available(path)?;
        let remaining = self
            .max_buffered_payload_bytes
            .checked_sub(self.stats.buffered_payload_bytes)
            .ok_or_else(|| {
                CliError::package_invalid(
                    "buffered package payload already exceeds max_buffered_payload_bytes",
                )
            })?;
        let mut writer =
            BoundedPayloadWriter::new(path, remaining, self.max_buffered_payload_bytes);
        serde_json::to_writer_pretty(&mut writer, &value).map_err(|err| {
            CliError::package_invalid(format!("failed to serialize {path}: {err}"))
        })?;
        writer.write_all(b"\n").map_err(|err| {
            CliError::package_invalid(format!("failed to serialize {path}: {err}"))
        })?;
        self.add_buffered(path, writer.into_inner())
    }

    fn add_file(&mut self, source: &Path, zip_path: &str) -> CliOutcome<()> {
        self.ensure_entry_available(zip_path)?;
        let metadata = inspect_approved_path(&self.approved_root, source)?.ok_or_else(|| {
            CliError::package_invalid(format!(
                "approved package source file does not exist: {}",
                source.display()
            ))
        })?;
        if !metadata.is_file() {
            return Err(CliError::package_invalid(format!(
                "approved package source path is not a regular file: {}",
                source.display()
            )));
        }
        let expected_len = metadata.len();
        let entry_len = usize::try_from(expected_len).map_err(|_| {
            CliError::package_invalid(format!(
                "package source entry {} is too large for this platform",
                source.display()
            ))
        })?;
        if entry_len > self.max_buffered_payload_bytes {
            return Err(CliError::package_invalid(format!(
                "package source entry {} is {entry_len} bytes, exceeding max_buffered_payload_bytes={}",
                source.display(),
                self.max_buffered_payload_bytes
            )));
        }
        let sha256 = stream_approved_source(
            &self.approved_root,
            source,
            expected_len,
            &mut io::sink(),
            &mut self.stats,
        )?;
        self.stats.add_logical(expected_len)?;
        self.files.insert(
            zip_path.to_string(),
            PackagePayload::SourceFile {
                source: source.to_path_buf(),
                expected_len,
                sha256,
            },
        );
        Ok(())
    }

    fn add_operation_dir(
        &mut self,
        environment: &AuthoringEnvironmentSnapshot,
        source_dir: &Path,
        zip_prefix: &str,
        resource_root: &Path,
        canonical_task: &Value,
    ) -> CliOutcome<()> {
        for path in collect_files(&self.approved_root, source_dir)? {
            let rel = relative_slash(source_dir, &path)?;
            if rel == "task.json" {
                let mut task = canonical_task.clone();
                environment.apply(&mut task)?;
                self.add_root_relative_operation_assets(&task, resource_root)?;
                self.add_json(&format!("{zip_prefix}/{rel}"), task)?;
            } else {
                self.add_file(&path, &format!("{zip_prefix}/{rel}"))?;
            }
        }
        Ok(())
    }

    fn add_root_relative_operation_assets(
        &mut self,
        task: &Value,
        resource_root: &Path,
    ) -> CliOutcome<()> {
        for path in task_verify_template_paths(task) {
            if path.starts_with("assets/") {
                continue;
            }
            if !is_safe_package_relative_path(path) {
                return Err(CliError::package_invalid(format!(
                    "operation verify_template path is unsafe: {path}"
                )));
            }
            let source = resource_root.join(path);
            if !approved_file_exists(&self.approved_root, &source)? {
                continue;
            }
            let zip_path = format!("resources/{path}");
            if !self.contains(&zip_path) {
                self.add_file(&source, &zip_path)?;
            }
        }
        Ok(())
    }

    fn add_manifest(&mut self, entry_task_id: &str) -> CliOutcome<()> {
        let files = self
            .files
            .iter()
            .filter(|(path, _)| {
                path.starts_with("resources/") && path.as_str() != "resources/manifest.json"
            })
            .map(|(path, payload)| {
                ordered_object([
                    (
                        "path",
                        Value::String(path.trim_start_matches("resources/").to_string()),
                    ),
                    (
                        "sha256",
                        Value::String(format!("sha256:{}", payload.sha256())),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let manifest = ordered_object([
            ("schema_version", Value::String("0.3".to_string())),
            ("entry_task_id", Value::String(entry_task_id.to_string())),
            ("files", Value::Array(files)),
        ]);
        self.add_json("resources/manifest.json", manifest)
    }

    fn add_buffered(&mut self, path: &str, bytes: Vec<u8>) -> CliOutcome<()> {
        self.ensure_entry_available(path)?;
        let next = self
            .stats
            .buffered_payload_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| {
                CliError::package_invalid("package buffered payload byte count overflow")
            })?;
        if next > self.max_buffered_payload_bytes {
            return Err(CliError::package_invalid(format!(
                "package entry {path} would raise buffered payload to {next} bytes, exceeding max_buffered_payload_bytes={}",
                self.max_buffered_payload_bytes
            )));
        }
        let sha256 = hex_sha256(&bytes);
        self.stats.add_buffered(bytes.len())?;
        self.files
            .insert(path.to_string(), PackagePayload::Buffered { bytes, sha256 });
        Ok(())
    }

    fn ensure_entry_available(&self, path: &str) -> CliOutcome<()> {
        validate_zip_entry_path(path)?;
        if self.files.contains_key(path) {
            return Err(CliError::package_invalid(format!(
                "duplicate package entry: {path}"
            )));
        }
        Ok(())
    }

    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn required_buffered(&self, path: &str) -> CliOutcome<&[u8]> {
        let payload = self
            .files
            .get(path)
            .ok_or_else(|| CliError::package_invalid(format!("missing package entry: {path}")))?;
        payload.buffered_bytes().ok_or_else(|| {
            CliError::package_invalid(format!(
                "package metadata entry must be buffered during validation: {path}"
            ))
        })
    }
}

fn validate_max_buffered_payload_bytes(max_buffered_payload_bytes: usize) -> CliOutcome<()> {
    if max_buffered_payload_bytes == 0 {
        return Err(CliError::package_invalid(
            "max_buffered_payload_bytes must be positive",
        ));
    }
    Ok(())
}

struct BoundedPayloadWriter {
    entry: String,
    bytes: Vec<u8>,
    remaining_budget: usize,
    total_budget: usize,
}

impl BoundedPayloadWriter {
    fn new(entry: &str, remaining_budget: usize, total_budget: usize) -> Self {
        Self {
            entry: entry.to_string(),
            bytes: Vec::new(),
            remaining_budget,
            total_budget,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedPayloadWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::other("generated package payload length overflow"))?;
        if next > self.remaining_budget {
            return Err(io::Error::other(format!(
                "package entry {} exceeds remaining buffered payload budget of {} bytes (max_buffered_payload_bytes={})",
                self.entry, self.remaining_budget, self.total_budget
            )));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct PackageWrite {
    validation: LabPackageValidationResponse,
    #[cfg(test)]
    stats: PackagePayloadStats,
}

fn write_and_validate_package(
    out: &Path,
    entries: PackageEntries,
    dry_run: bool,
) -> CliOutcome<PackageWrite> {
    if dry_run {
        return write_and_validate_dry_run(out, entries);
    }
    let transaction = PackagePublicationTransaction::begin_single(out)?;
    let staged = match transaction.staging_path(out) {
        Ok(staged) => staged,
        Err(error) => {
            return Err(match transaction.abort() {
                Ok(()) => error,
                Err(cleanup) => combine_package_errors(error, cleanup),
            });
        }
    };
    let mut write = match write_and_validate_package_staged(&staged, entries, false) {
        Ok(write) => write,
        Err(error) => {
            return Err(match transaction.abort() {
                Ok(()) => error,
                Err(cleanup) => combine_package_errors(error, cleanup),
            });
        }
    };
    transaction.commit()?;
    write.validation.zip = out.display().to_string();
    Ok(write)
}

fn write_and_validate_package_staged(
    out: &Path,
    mut entries: PackageEntries,
    dry_run: bool,
) -> CliOutcome<PackageWrite> {
    if dry_run {
        return write_and_validate_dry_run(out, entries);
    }
    let mut validation = build_validated_package(out, &mut entries)?;
    validation.zip = out.display().to_string();
    Ok(PackageWrite {
        validation,
        #[cfg(test)]
        stats: entries.stats,
    })
}

fn write_and_validate_dry_run(out: &Path, mut entries: PackageEntries) -> CliOutcome<PackageWrite> {
    let temp = temp_zip_path(out)?;
    let mut validation = build_validated_package(&temp, &mut entries)?;
    fs::remove_file(&temp).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to remove dry-run package {}: {error}",
            temp.display()
        ))
    })?;
    validation.zip = out.display().to_string();
    Ok(PackageWrite {
        validation,
        #[cfg(test)]
        stats: entries.stats,
    })
}

fn build_validated_package(
    path: &Path,
    entries: &mut PackageEntries,
) -> CliOutcome<LabPackageValidationResponse> {
    if path.exists() {
        return Err(CliError::package_invalid(format!(
            "refusing to overwrite staged package output: {}",
            path.display()
        )));
    }
    if let Err(error) = write_zip(path, entries) {
        return Err(cleanup_failed_package(path, error));
    }
    match validate_generated_package(path, entries) {
        Ok(validation) => Ok(validation),
        Err(error) => Err(cleanup_failed_package(path, error)),
    }
}

fn cleanup_failed_package(path: &Path, primary: CliError) -> CliError {
    match fs::remove_file(path) {
        Ok(()) => primary,
        Err(error) if error.kind() == io::ErrorKind::NotFound => primary,
        Err(error) => combine_package_errors(
            primary,
            CliError::package_invalid(format!(
                "failed to remove invalid staged package {}: {error}",
                path.display()
            )),
        ),
    }
}

fn combine_package_errors(mut primary: CliError, secondary: CliError) -> CliError {
    primary.message = format!(
        "{}; secondary_failure={}",
        primary.message, secondary.message
    );
    primary
}

fn write_zip(path: &Path, entries: &mut PackageEntries) -> CliOutcome<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", path.display()))
        })?;
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(file);
    let PackageEntries {
        approved_root,
        files,
        stats,
        ..
    } = entries;
    for (entry, payload) in files {
        zip.start_file(entry, options).map_err(zip_write_error)?;
        match payload {
            PackagePayload::Buffered { bytes, .. } => {
                zip.write_all(bytes).map_err(zip_io_error)?;
            }
            PackagePayload::SourceFile {
                source,
                expected_len,
                sha256,
            } => {
                let written_sha256 =
                    stream_approved_source(approved_root, source, *expected_len, &mut zip, stats)?;
                if written_sha256 != *sha256 {
                    return Err(CliError::package_invalid(format!(
                        "package source file changed while building: {}",
                        source.display()
                    )));
                }
            }
        }
    }
    let file = zip.finish().map_err(zip_write_error)?;
    file.sync_all().map_err(|error| {
        CliError::package_invalid(format!(
            "failed to sync package output {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

fn stream_approved_source<W: Write>(
    approved_root: &Path,
    source: &Path,
    expected_len: u64,
    output: &mut W,
    stats: &mut PackagePayloadStats,
) -> CliOutcome<String> {
    let metadata = inspect_approved_path(approved_root, source)?.ok_or_else(|| {
        CliError::package_invalid(format!(
            "approved package source file does not exist: {}",
            source.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CliError::package_invalid(format!(
            "approved package source path is not a regular file: {}",
            source.display()
        )));
    }
    if metadata.len() != expected_len {
        return Err(CliError::package_invalid(format!(
            "package source file size changed before streaming: {} (expected {expected_len}, found {})",
            source.display(),
            metadata.len()
        )));
    }

    let mut file = File::open(source).map_err(|err| {
        CliError::package_invalid(format!("failed to open {}: {err}", source.display()))
    })?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", source.display()))
        })?;
        if read == 0 {
            break;
        }
        stats.observe_streamed(read)?;
        total = total
            .checked_add(u64::try_from(read).map_err(|_| {
                CliError::package_invalid("streamed package chunk does not fit in u64")
            })?)
            .ok_or_else(|| CliError::package_invalid("streamed package byte count overflow"))?;
        if total > expected_len {
            return Err(CliError::package_invalid(format!(
                "package source file grew while streaming: {} (expected {expected_len} bytes)",
                source.display()
            )));
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to stream package source {}: {err}",
                source.display()
            ))
        })?;
    }
    if total != expected_len {
        return Err(CliError::package_invalid(format!(
            "package source file was truncated while streaming: {} (expected {expected_len}, read {total})",
            source.display()
        )));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_generated_package(
    path: &Path,
    entries: &mut PackageEntries,
) -> CliOutcome<LabPackageValidationResponse> {
    let entry_count = validate_generated_archive(path, entries)?;
    let control_value = parse_buffered_json(entries, "control.json")?;
    let control: LabControl = serde_json::from_value(control_value).map_err(|error| {
        CliError::package_invalid(format!("failed to parse control.json: {error}"))
    })?;
    control.validate()?;

    let resource_root = "resources";
    let manifest_path = format!("{resource_root}/manifest.json");
    let manifest = parse_buffered_json(entries, &manifest_path)?;
    validate_manifest_entry_task_id(Path::new(&manifest_path), &manifest, &control)?;
    validate_generated_manifest_hashes(&manifest, entries, resource_root)?;

    let operation_path = format!(
        "{resource_root}/operations/{}/task.json",
        control.entry_task_id
    );
    let operation_value = parse_buffered_json(entries, &operation_path)?;
    let operation_bundle: OperationBundle =
        serde_json::from_value(operation_value).map_err(|error| {
            CliError::package_invalid(format!("failed to parse {operation_path}: {error}"))
        })?;
    operation_bundle.validate(&control, |relative| {
        Ok(entries.contains(&format!(
            "{resource_root}/operations/{}/{}",
            control.entry_task_id, relative
        )))
    })?;
    validate_generated_post_admission_ocr(path, entries, &control, &operation_bundle)?;

    let stem = format!("{}.{}", control.game, control.server);
    let pack = format!("{resource_root}/recognition/{stem}.pack.json");
    let pages = format!("{resource_root}/recognition/{stem}.pages.json");
    let pack_json = buffered_utf8(entries, &pack)?;
    let pages_json = buffered_utf8(entries, &pages)?;
    let resolver = Arc::new(PackageEntryAssetResolver {
        paths: entries.files.keys().cloned().collect(),
        resource_root: resource_root.to_string(),
    });
    let unsupported_targets =
        validate_recognition_metadata(&pack, pack_json, &pages, pages_json, resolver)
            .map_err(containment_error)?
            .into_iter()
            .map(|target| UnsupportedRecognitionTargetResponse {
                id: target.id,
                reason: target.reason,
            })
            .collect::<Vec<_>>();
    let navigation = format!("{resource_root}/navigation/{stem}.navigation.json");
    let navigation = if entries.contains(&navigation) {
        parse_buffered_json(entries, &navigation)?;
        Some(navigation)
    } else {
        None
    };

    Ok(LabPackageValidationResponse {
        zip: path.display().to_string(),
        status: "valid".to_string(),
        entry_count,
        control: LabPackageControlResponse {
            package_id: control.package_id,
            execution_mode: control.execution_mode,
            game: control.game,
            server: control.server,
            resolution: PackageResolution {
                width: control.resolution.width,
                height: control.resolution.height,
            },
            entry_task_id: control.entry_task_id,
        },
        resources: LabPackageResourcesResponse {
            resource_root: resource_root.to_string(),
            manifest: manifest_path,
            operation: operation_path,
            operation_count: operation_bundle.operations.len(),
            pack,
            recognition_unsupported_target_count: unsupported_targets.len(),
            recognition_unsupported_targets: unsupported_targets,
            pages,
            navigation,
        },
    })
}

fn validate_generated_post_admission_ocr(
    archive_path: &Path,
    entries: &PackageEntries,
    control: &LabControl,
    operation: &OperationBundle,
) -> CliOutcome<()> {
    let Some(declaration) = operation.post_admission_ocr.as_ref() else {
        return Ok(());
    };
    let entry_path = format!(
        "resources/operations/{}/{}",
        control.entry_task_id, declaration.truth_set.path
    );
    let payload = entries.files.get(&entry_path).ok_or_else(|| {
        CliError::package_invalid(format!(
            "post_admission_ocr truth set is missing from generated package: {entry_path}"
        ))
    })?;
    if payload.sha256() != declaration.truth_set.sha256 {
        return Err(CliError::package_invalid(format!(
            "post_admission_ocr truth set hash mismatch for {entry_path}"
        )));
    }
    let file = File::open(archive_path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to open generated package {}: {error}",
            archive_path.display()
        ))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to inspect generated package {}: {error}",
            archive_path.display()
        ))
    })?;
    let mut entry = archive.by_name(&entry_path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to read post_admission_ocr truth set {entry_path}: {error}"
        ))
    })?;
    if entry.size() > declaration.limits.max_total_bytes {
        return Err(CliError::package_invalid(
            "post_admission_ocr truth set exceeds max_total_bytes",
        ));
    }
    let capacity = usize::try_from(entry.size()).map_err(|_| {
        CliError::package_invalid("post_admission_ocr truth set size does not fit in memory")
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    entry.read_to_end(&mut bytes).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to read post_admission_ocr truth set {entry_path}: {error}"
        ))
    })?;
    if format!("{:x}", Sha256::digest(&bytes)) != declaration.truth_set.sha256 {
        return Err(CliError::package_invalid(
            "post_admission_ocr truth set bytes do not match the declared SHA-256",
        ));
    }
    let truth: PostAdmissionOcrPackageTruthSet =
        serde_json::from_slice(&bytes).map_err(|error| {
            CliError::package_invalid(format!(
                "post_admission_ocr truth set is invalid JSON: {error}"
            ))
        })?;
    if truth.schema_version != "actingcommand.ocr-truth-set.v1"
        || truth.items.is_empty()
        || u32::try_from(truth.items.len())
            .map_or(true, |count| count > declaration.limits.max_truth_entries)
    {
        return Err(CliError::package_invalid(
            "post_admission_ocr truth set schema or item count is invalid",
        ));
    }
    let max_string_bytes = usize::try_from(declaration.limits.max_string_bytes)
        .map_err(|_| CliError::package_invalid("post_admission_ocr string bound is invalid"))?;
    let mut normalized = BTreeSet::new();
    for item in truth.items {
        let value = item.trim().to_lowercase();
        if item.len() > max_string_bytes
            || value.is_empty()
            || value.len() > max_string_bytes
            || !normalized.insert(value)
        {
            return Err(CliError::package_invalid(
                "post_admission_ocr truth items are empty, oversized, or duplicated",
            ));
        }
    }
    Ok(())
}

fn validate_generated_archive(path: &Path, entries: &mut PackageEntries) -> CliOutcome<usize> {
    let limits = ContainmentLimits::default();
    let metadata = fs::metadata(path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to inspect generated package {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CliError::package_invalid(format!(
            "generated package is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > limits.max_compressed_bytes {
        return Err(CliError::package_invalid(format!(
            "generated package {} is {} bytes, exceeding compressed limit {}",
            path.display(),
            metadata.len(),
            limits.max_compressed_bytes
        )));
    }
    let file = File::open(path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to open generated package {}: {error}",
            path.display()
        ))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to parse generated package {}: {error}",
            path.display()
        ))
    })?;
    if archive.len() > limits.max_entry_count {
        return Err(CliError::package_invalid(format!(
            "generated package entry count {} exceeds limit {}",
            archive.len(),
            limits.max_entry_count
        )));
    }

    let mut seen = BTreeSet::new();
    let mut total_decompressed = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to inspect generated package entry {index}: {error}"
            ))
        })?;
        let entry_path = entry.name().to_string();
        validate_zip_entry_path(&entry_path)?;
        if !seen.insert(entry_path.to_ascii_lowercase()) {
            return Err(CliError::package_invalid(format!(
                "duplicate package entry: {entry_path}"
            )));
        }
        let expected = entries.files.get(&entry_path).ok_or_else(|| {
            CliError::package_invalid(format!(
                "generated package contains unexpected entry: {entry_path}"
            ))
        })?;
        let expected_len = expected.expected_len()?;
        if entry.size() != expected_len {
            return Err(CliError::package_invalid(format!(
                "generated package entry length mismatch for {entry_path}: expected {expected_len}, found {}",
                entry.size()
            )));
        }
        if entry.size()
            > u64::try_from(entries.max_buffered_payload_bytes).map_err(|_| {
                CliError::package_invalid("max_buffered_payload_bytes does not fit in u64")
            })?
        {
            return Err(CliError::package_invalid(format!(
                "generated package entry {entry_path} is {} bytes, exceeding max_buffered_payload_bytes={}",
                entry.size(),
                entries.max_buffered_payload_bytes
            )));
        }
        if entry.size() > limits.max_entry_bytes {
            return Err(CliError::package_invalid(format!(
                "generated package entry {entry_path} is {} bytes, exceeding entry limit {}",
                entry.size(),
                limits.max_entry_bytes
            )));
        }
        total_decompressed = total_decompressed
            .checked_add(entry.size())
            .ok_or_else(|| CliError::package_invalid("package decompressed size overflow"))?;
        if total_decompressed > limits.max_total_decompressed_bytes {
            return Err(CliError::package_invalid(format!(
                "generated package decompressed size {total_decompressed} exceeds limit {}",
                limits.max_total_decompressed_bytes
            )));
        }
        let actual_sha256 = stream_zip_entry_hash(&mut entry, &entry_path, &mut entries.stats)?;
        if actual_sha256 != expected.sha256() {
            return Err(CliError::package_invalid(format!(
                "generated package entry hash mismatch for {entry_path}: expected {}, found {actual_sha256}",
                expected.sha256()
            )));
        }
    }

    if seen.len() != entries.files.len() {
        let missing = entries
            .files
            .keys()
            .find(|path| !seen.contains(&path.to_ascii_lowercase()))
            .expect("entry count mismatch implies a missing expected path");
        return Err(CliError::package_invalid(format!(
            "generated package is missing expected entry: {missing}"
        )));
    }
    Ok(archive.len())
}

fn stream_zip_entry_hash<R: Read>(
    reader: &mut R,
    path: &str,
    stats: &mut PackagePayloadStats,
) -> CliOutcome<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to stream generated package entry {path}: {error}"
            ))
        })?;
        if read == 0 {
            break;
        }
        stats.observe_validation_streamed(read)?;
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_buffered_json(entries: &PackageEntries, path: &str) -> CliOutcome<Value> {
    serde_json::from_slice(entries.required_buffered(path)?)
        .map_err(|error| CliError::package_invalid(format!("failed to parse {path}: {error}")))
}

fn buffered_utf8<'a>(entries: &'a PackageEntries, path: &str) -> CliOutcome<&'a str> {
    std::str::from_utf8(entries.required_buffered(path)?).map_err(|error| {
        CliError::package_invalid(format!("failed to decode {path} as UTF-8: {error}"))
    })
}

#[derive(Debug, Deserialize)]
struct GeneratedManifestFile {
    path: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeneratedManifestHashes {
    #[serde(default)]
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<GeneratedManifestFile>,
}

fn validate_generated_manifest_hashes(
    manifest: &Value,
    entries: &PackageEntries,
    resource_root: &str,
) -> CliOutcome<()> {
    let hashes: GeneratedManifestHashes =
        serde_json::from_value(manifest.clone()).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to parse {resource_root}/manifest.json hashes: {error}"
            ))
        })?;
    for (path, expected) in hashes
        .hashes
        .iter()
        .map(|(path, hash)| (path.as_str(), hash.as_str()))
        .chain(hashes.files.iter().filter_map(|file| {
            file.sha256
                .as_deref()
                .or(file.hash.as_deref())
                .map(|hash| (file.path.as_str(), hash))
        }))
    {
        if !is_safe_package_relative_path(path) {
            return Err(CliError::package_invalid(format!(
                "unsafe manifest hash path: {path}"
            )));
        }
        let resolved = if entries.contains(path) {
            path.to_string()
        } else {
            format!("{resource_root}/{path}")
        };
        let payload = entries.files.get(&resolved).ok_or_else(|| {
            CliError::package_invalid(format!("missing package entry: {resolved}"))
        })?;
        let expected = Sha256Hash::parse_hex(expected).map_err(containment_error)?;
        if payload.sha256() != expected.to_string() {
            return Err(CliError::package_invalid(format!(
                "manifest hash mismatch for {resolved}: expected {expected}, actual {}",
                payload.sha256()
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PackageEntryAssetResolver {
    paths: BTreeSet<String>,
    resource_root: String,
}

impl AssetResolver for PackageEntryAssetResolver {
    fn read_asset(&self, path: &str) -> Result<Vec<u8>, RecognitionPackError> {
        Err(RecognitionPackError::fatal(format!(
            "bounded package validation does not materialize asset '{path}'"
        )))
    }

    fn contains_asset(&self, path: &str) -> bool {
        is_safe_package_relative_path(path)
            && (self.paths.contains(path)
                || self.paths.contains(&format!(
                    "{}/{}",
                    self.resource_root.trim_end_matches('/'),
                    path.trim_start_matches('/')
                )))
    }
}

fn validate_manifest_entry_task_id(
    manifest_path: &Path,
    manifest: &Value,
    control: &LabControl,
) -> CliOutcome<()> {
    let Some(value) = manifest.get("entry_task_id") else {
        return Ok(());
    };
    let Some(manifest_entry_task_id) = value.as_str() else {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id must be a string when present",
            manifest_path.display()
        )));
    };
    if manifest_entry_task_id != control.entry_task_id {
        return Err(CliError::package_invalid(format!(
            "{} entry_task_id '{}' conflicts with control entry_task_id '{}'",
            manifest_path.display(),
            manifest_entry_task_id,
            control.entry_task_id
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StabilityTermination {
    region: StabilityTerminationRegion,
    comparison: StabilityTerminationComparison,
    consecutive_unchanged_threshold: u32,
    max_steps: u32,
}

impl StabilityTermination {
    fn validate(&self, resolution: (u32, u32)) -> CliOutcome<()> {
        self.region.validate(resolution)?;
        if self.max_steps < 2 || self.max_steps > MAX_STABILITY_TERMINATION_STEPS {
            return Err(CliError::package_invalid(format!(
                "stability_termination max_steps must be between 2 and {MAX_STABILITY_TERMINATION_STEPS}"
            )));
        }
        if self.consecutive_unchanged_threshold == 0
            || self.consecutive_unchanged_threshold >= self.max_steps
        {
            return Err(CliError::package_invalid(
                "stability_termination consecutive_unchanged_threshold must be positive and less than max_steps",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StabilityTerminationRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl StabilityTerminationRegion {
    fn validate(self, resolution: (u32, u32)) -> CliOutcome<()> {
        if self.width == 0 || self.height == 0 {
            return Err(CliError::package_invalid(
                "stability_termination region width and height must be non-zero",
            ));
        }
        let end_x = self.x.checked_add(self.width).ok_or_else(|| {
            CliError::package_invalid("stability_termination region x extent overflow")
        })?;
        let end_y = self.y.checked_add(self.height).ok_or_else(|| {
            CliError::package_invalid("stability_termination region y extent overflow")
        })?;
        if end_x > resolution.0 || end_y > resolution.1 {
            return Err(CliError::package_invalid(format!(
                "stability_termination region ({},{}) {}x{} exceeds resolution {}x{}",
                self.x, self.y, self.width, self.height, resolution.0, resolution.1
            )));
        }
        validate_stability_crop_byte_layout(self, resolution)
    }
}

fn validate_stability_crop_byte_layout(
    region: StabilityTerminationRegion,
    resolution: (u32, u32),
) -> CliOutcome<()> {
    let frame_width = usize::try_from(resolution.0).map_err(|_| {
        CliError::package_invalid("stability_termination frame width exceeds platform limits")
    })?;
    let frame_height = usize::try_from(resolution.1).map_err(|_| {
        CliError::package_invalid("stability_termination frame height exceeds platform limits")
    })?;
    let x = usize::try_from(region.x).map_err(|_| {
        CliError::package_invalid("stability_termination region x exceeds platform limits")
    })?;
    let y = usize::try_from(region.y).map_err(|_| {
        CliError::package_invalid("stability_termination region y exceeds platform limits")
    })?;
    let width = usize::try_from(region.width).map_err(|_| {
        CliError::package_invalid("stability_termination region width exceeds platform limits")
    })?;
    let height = usize::try_from(region.height).map_err(|_| {
        CliError::package_invalid("stability_termination region height exceeds platform limits")
    })?;
    let frame_stride = frame_width
        .checked_mul(MAX_CAPTURE_PIXEL_BYTES)
        .ok_or_else(|| {
            CliError::package_invalid("stability_termination frame byte-stride overflow")
        })?;
    let frame_bytes = frame_stride.checked_mul(frame_height).ok_or_else(|| {
        CliError::package_invalid("stability_termination frame byte-layout overflow")
    })?;
    let row_bytes = width.checked_mul(MAX_CAPTURE_PIXEL_BYTES).ok_or_else(|| {
        CliError::package_invalid("stability_termination crop row byte-layout overflow")
    })?;
    let last_row = y.checked_add(height - 1).ok_or_else(|| {
        CliError::package_invalid("stability_termination crop row offset overflow")
    })?;
    let last_row_start = last_row
        .checked_mul(frame_stride)
        .and_then(|offset| {
            x.checked_mul(MAX_CAPTURE_PIXEL_BYTES)
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .ok_or_else(|| {
            CliError::package_invalid("stability_termination crop byte offset overflow")
        })?;
    let crop_end = last_row_start.checked_add(row_bytes).ok_or_else(|| {
        CliError::package_invalid("stability_termination crop byte extent overflow")
    })?;
    if crop_end > frame_bytes {
        return Err(CliError::package_invalid(
            "stability_termination crop byte extent exceeds frame layout",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StabilityTerminationComparison {
    mode: StabilityTerminationComparisonMode,
    parameters: ExactPixelsV1Parameters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StabilityTerminationComparisonMode {
    ExactPixelsV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct ExactPixelsV1Parameters {}

impl<'de> Deserialize<'de> for ExactPixelsV1Parameters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EmptyObjectVisitor;

        impl<'de> serde::de::Visitor<'de> for EmptyObjectVisitor {
            type Value = ExactPixelsV1Parameters;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an empty exact_pixels_v1 parameters object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                if let Some(key) = map.next_key::<String>()? {
                    let _: serde::de::IgnoredAny = map.next_value()?;
                    return Err(serde::de::Error::unknown_field(&key, &[]));
                }
                Ok(ExactPixelsV1Parameters {})
            }
        }

        deserializer.deserialize_map(EmptyObjectVisitor)
    }
}

fn validate_entry_stability_termination(
    bundle: &Bundle,
    execution_mode: &str,
    resolution: (u32, u32),
) -> CliOutcome<Option<StabilityTermination>> {
    let declaration = bundle
        .data
        .get("stability_termination")
        .map(|value| {
            serde_json::from_value::<StabilityTermination>(value.clone()).map_err(|error| {
                CliError::package_invalid(format!(
                    "task '{}' stability_termination is invalid: {error}",
                    bundle.task_id
                ))
            })
        })
        .transpose()?;
    match (execution_mode, declaration.as_ref()) {
        ("in_page_guard", None) => {}
        ("in_page_guard", Some(declaration)) => {
            declaration.validate(resolution)?;
            if bundle
                .data
                .get("target_page")
                .is_some_and(|value| !value.is_null())
            {
                return Err(CliError::package_invalid(format!(
                    "task '{}' in_page_guard stability_termination conflicts with target_page",
                    bundle.task_id
                )));
            }
            if let Some(scheduling_outcome) = bundle
                .data
                .get("scheduling_outcome")
                .filter(|value| !value.is_null())
            {
                let post_admission_ocr: PostAdmissionOcrPackageDeclaration = bundle
                    .data
                    .get("post_admission_ocr")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|error| {
                        CliError::package_invalid(format!(
                            "task '{}' post_admission_ocr is invalid: {error}",
                            bundle.task_id
                        ))
                    })?
                    .ok_or_else(|| {
                        CliError::package_invalid(format!(
                            "task '{}' in_page_guard stability_termination conflicts with scheduling_outcome",
                            bundle.task_id
                        ))
                    })?;
                post_admission_ocr.validate(Some(scheduling_outcome))?;
            }
        }
        (_, Some(_)) => {
            return Err(CliError::package_invalid(format!(
                "task '{}' stability_termination is only valid for in_page_guard execution",
                bundle.task_id
            )));
        }
        (_, None) => {}
    }
    Ok(declaration)
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct LabControl {
    schema_version: String,
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    #[serde(default)]
    capture_interval_ms: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    step_timeout_ms: Option<u64>,
    #[serde(default)]
    max_steps: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    stability_termination: Option<StabilityTermination>,
    #[serde(default)]
    stop_on_error: Option<bool>,
    #[serde(default)]
    stop_on_confirmation: Option<bool>,
    #[serde(default)]
    allow_placeholder_coords: Option<bool>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    capture_backend: Option<String>,
    #[serde(default)]
    frame_store: FrameStoreControl,
    #[serde(default)]
    producer: Option<Value>,
    #[serde(default)]
    trusted_execution: Option<Value>,
}

impl LabControl {
    fn validate(&self) -> CliOutcome<()> {
        if self.schema_version != CONTROL_SCHEMA {
            return Err(CliError::package_invalid(format!(
                "unsupported control schema_version '{}', expected {CONTROL_SCHEMA}",
                self.schema_version
            )));
        }
        if !matches!(
            self.execution_mode.as_str(),
            "navigable_route" | "recognize_only" | "in_page_guard"
        ) {
            return Err(CliError::package_invalid(format!(
                "unsupported execution_mode '{}', expected navigable_route, recognize_only, or in_page_guard",
                self.execution_mode
            )));
        }
        for (name, value) in [
            ("package_id", &self.package_id),
            ("game", &self.game),
            ("server", &self.server),
            ("entry_task_id", &self.entry_task_id),
        ] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "control {name} is empty"
                )));
            }
        }
        if self.resolution.width == 0 || self.resolution.height == 0 {
            return Err(CliError::package_invalid(
                "control resolution width and height must be non-zero",
            ));
        }
        if self.capture_interval_ms == Some(0) {
            return Err(CliError::package_invalid(
                "capture_interval_ms must be positive when provided",
            ));
        }
        self.validate_stability_termination()?;
        if let Some(capture_backend) = &self.capture_backend {
            validate_capture_backend(capture_backend)?;
        }
        self.frame_store
            .validate()
            .map_err(CliError::package_invalid)
    }

    fn validate_stability_termination(&self) -> CliOutcome<()> {
        match (
            self.execution_mode.as_str(),
            self.stability_termination.as_ref(),
        ) {
            ("in_page_guard", None) => Ok(()),
            ("in_page_guard", Some(declaration)) => {
                declaration.validate((self.resolution.width, self.resolution.height))?;
                let declared_max_steps = usize::try_from(declaration.max_steps).map_err(|_| {
                    CliError::package_invalid(
                        "stability_termination max_steps exceeds platform limits",
                    )
                })?;
                if self.max_steps != Some(declared_max_steps) {
                    return Err(CliError::package_invalid(format!(
                        "control max_steps {:?} does not match stability_termination max_steps {}",
                        self.max_steps, declaration.max_steps
                    )));
                }
                Ok(())
            }
            (_, Some(_)) => Err(CliError::package_invalid(
                "control stability_termination is only valid for in_page_guard execution",
            )),
            (_, None) => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrPackageTruthReference {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrPackageLimits {
    max_frames: u32,
    max_items: u32,
    max_string_bytes: u32,
    max_total_bytes: u64,
    max_truth_entries: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrPackageDeclaration {
    page_id: String,
    target_id: String,
    truth_set: PostAdmissionOcrPackageTruthReference,
    normalization: String,
    comparison: String,
    limits: PostAdmissionOcrPackageLimits,
    outcome_key: String,
}

impl PostAdmissionOcrPackageDeclaration {
    fn validate(&self, scheduling_outcome: Option<&Value>) -> CliOutcome<()> {
        let limits = &self.limits;
        if self.page_id.trim().is_empty()
            || self.target_id.trim().is_empty()
            || self.outcome_key.trim().is_empty()
            || self.normalization != "trim_lowercase_v1"
            || self.comparison != "exact_set_v1"
            || !is_safe_package_relative_path(&self.truth_set.path)
            || self.truth_set.sha256.len() != 64
            || !self
                .truth_set
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || limits.max_frames == 0
            || limits.max_frames > 256
            || limits.max_items == 0
            || limits.max_items > 4_096
            || limits.max_string_bytes == 0
            || limits.max_string_bytes > 4_096
            || limits.max_total_bytes == 0
            || limits.max_total_bytes > 4 * 1024 * 1024
            || limits.max_truth_entries == 0
            || limits.max_truth_entries > 4_096
        {
            return Err(CliError::package_invalid(
                "post_admission_ocr declaration is invalid",
            ));
        }
        let scheduling: SchedulingOutcomeDeclaration =
            serde_json::from_value(scheduling_outcome.cloned().ok_or_else(|| {
                CliError::package_invalid("post_admission_ocr requires scheduling_outcome")
            })?)
            .map_err(|_| {
                CliError::package_invalid("post_admission_ocr scheduling_outcome is invalid")
            })?;
        scheduling.validate().map_err(|_| {
            CliError::package_invalid("post_admission_ocr scheduling_outcome is invalid")
        })?;
        if scheduling
            .mappings()
            .iter()
            .filter(|mapping| mapping.outcome_key() == self.outcome_key)
            .count()
            != 1
        {
            return Err(CliError::package_invalid(
                "post_admission_ocr outcome_key is not one scheduling mapping",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostAdmissionOcrPackageTruthSet {
    schema_version: String,
    items: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct OperationBundle {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    #[serde(default)]
    goal: String,
    coordinate_space: Resolution,
    #[serde(default)]
    defaults: OperationDefaults,
    #[serde(default)]
    anchors: Vec<OperationAnchor>,
    #[serde(default)]
    entry_page: Option<String>,
    #[serde(default)]
    target_page: Option<PageDeclaration>,
    #[serde(default)]
    scheduling_outcome: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    post_admission_ocr: Option<PostAdmissionOcrPackageDeclaration>,
    #[serde(default, deserialize_with = "deserialize_non_null_option")]
    stability_termination: Option<StabilityTermination>,
    #[serde(default)]
    error_pages: Vec<String>,
    #[serde(default)]
    recovery: Option<TaskRecovery>,
    #[serde(default)]
    max_task_retries: Option<u32>,
    #[serde(default)]
    on_exhausted: Option<String>,
    #[serde(default)]
    page_rules: BTreeMap<String, Value>,
    operations: Vec<Operation>,
}

impl OperationBundle {
    fn validate(
        &self,
        control: &LabControl,
        mut operation_asset_exists: impl FnMut(&str) -> CliOutcome<bool>,
    ) -> CliOutcome<()> {
        let schema_valid = match self.schema_version.as_str() {
            "0.3" | "0.4" | "0.5" | "0.6" => self.post_admission_ocr.is_none(),
            "0.7" => self.post_admission_ocr.is_some(),
            _ => false,
        };
        if !schema_valid {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}' or post_admission_ocr/schema mismatch",
                self.schema_version
            )));
        }
        if self.task_id != control.entry_task_id && self.task_id != "return_home" {
            return Err(CliError::package_invalid(format!(
                "operation task_id '{}' does not match control entry_task_id '{}'",
                self.task_id, control.entry_task_id
            )));
        }
        if self.game != control.game {
            return Err(CliError::package_invalid(format!(
                "operation game '{}' does not match control game '{}'",
                self.game, control.game
            )));
        }
        if !self.server_scope.is_empty()
            && !self
                .server_scope
                .iter()
                .any(|server| server == &control.server)
        {
            return Err(CliError::package_invalid(format!(
                "operation server_scope does not include '{}'",
                control.server
            )));
        }
        if self.coordinate_space.width != control.resolution.width
            || self.coordinate_space.height != control.resolution.height
        {
            return Err(CliError::package_invalid(format!(
                "operation coordinate_space {}x{} does not match control resolution {}x{}",
                self.coordinate_space.width,
                self.coordinate_space.height,
                control.resolution.width,
                control.resolution.height
            )));
        }
        if self.operations.is_empty() {
            return Err(CliError::package_invalid(
                "operation bundle has no operations",
            ));
        }
        self.validate_stability_termination(control)?;
        self.defaults.validate()?;
        if let Some(target_page) = &self.target_page {
            target_page.validate("operation bundle target_page")?;
        }
        if let Some(declaration) = &self.post_admission_ocr {
            declaration.validate(self.scheduling_outcome.as_ref())?;
        }
        for anchor in &self.anchors {
            if anchor.id.trim().is_empty() {
                return Err(CliError::package_invalid(
                    "operation anchor id must not be empty",
                ));
            }
            if !operation_asset_exists(&anchor.template)? {
                return Err(CliError::package_invalid(format!(
                    "operation anchor '{}' references missing template {}",
                    anchor.id, anchor.template
                )));
            }
        }
        let mut ids = BTreeSet::new();
        for operation in &self.operations {
            operation.validate(control)?;
            if !ids.insert(operation.id.clone()) {
                return Err(CliError::package_invalid(format!(
                    "duplicate operation id '{}'",
                    operation.id
                )));
            }
            if let Some(template) = &operation.verify_template
                && !operation_asset_exists(template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' references missing verify_template {}",
                    operation.id, template
                )));
            }
            if let Some(guard_template) = operation
                .guard
                .as_ref()
                .and_then(|guard| guard.verify_template.as_ref())
                && !matches!(
                    operation.click.kind.as_str(),
                    "offset" | "target" | "target_center"
                )
                && !operation_asset_exists(guard_template)?
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' guard references missing verify_template {}",
                    operation.id, guard_template
                )));
            }
        }
        self.validate_recovery()
    }

    fn validate_stability_termination(&self, control: &LabControl) -> CliOutcome<()> {
        if self.stability_termination != control.stability_termination {
            return Err(CliError::package_invalid(
                "task stability_termination does not exactly match control stability_termination",
            ));
        }
        match (
            control.execution_mode.as_str(),
            self.stability_termination.as_ref(),
        ) {
            ("in_page_guard", Some(declaration)) => {
                declaration
                    .validate((self.coordinate_space.width, self.coordinate_space.height))?;
                if self.target_page.is_some() {
                    return Err(CliError::package_invalid(
                        "in_page_guard stability_termination conflicts with target_page",
                    ));
                }
                if self.scheduling_outcome.is_some() && self.post_admission_ocr.is_none() {
                    return Err(CliError::package_invalid(
                        "in_page_guard stability_termination conflicts with scheduling_outcome",
                    ));
                }
                Ok(())
            }
            ("in_page_guard", None) => Ok(()),
            (_, Some(_)) => Err(CliError::package_invalid(
                "task stability_termination is only valid for in_page_guard execution",
            )),
            (_, None) => Ok(()),
        }
    }

    fn validate_recovery(&self) -> CliOutcome<()> {
        if self.max_task_retries == Some(0) {
            return Err(CliError::package_invalid(
                "operation bundle max_task_retries must be positive when provided",
            ));
        }
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
        }
        if let Some(on_exhausted) = &self.on_exhausted
            && on_exhausted != "pause"
        {
            return Err(CliError::package_invalid(format!(
                "operation bundle on_exhausted '{on_exhausted}' is unsupported; expected pause"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TaskRecovery {
    Kind(String),
    Config {
        kind: String,
        #[serde(default)]
        task_id: Option<String>,
    },
}

impl TaskRecovery {
    fn validate(&self) -> CliOutcome<()> {
        if self.kind() != "return_home" {
            return Err(CliError::package_invalid(format!(
                "operation bundle recovery kind '{}' is unsupported; expected return_home",
                self.kind()
            )));
        }
        if self.task_id().trim().is_empty() {
            return Err(CliError::package_invalid(
                "operation bundle recovery task_id must not be empty",
            ));
        }
        Ok(())
    }

    fn kind(&self) -> &str {
        match self {
            Self::Kind(kind) | Self::Config { kind, .. } => kind,
        }
    }

    fn task_id(&self) -> &str {
        match self {
            Self::Kind(_) => DEFAULT_RECOVERY_TASK_ID,
            Self::Config { task_id, .. } => task_id.as_deref().unwrap_or(DEFAULT_RECOVERY_TASK_ID),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize)]
struct OperationDefaults {
    #[serde(default = "default_template_threshold")]
    template_threshold: f32,
    #[serde(default)]
    color_max_distance: Option<f32>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    pre_delay_ms: Option<u64>,
    #[serde(default)]
    post_delay_ms: Option<u64>,
    #[serde(default)]
    pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    post_wait_freezes_ms: Option<u64>,
}

impl Default for OperationDefaults {
    fn default() -> Self {
        Self {
            template_threshold: DEFAULT_TEMPLATE_THRESHOLD,
            color_max_distance: None,
            timeout_ms: None,
            max_attempts: None,
            retry_interval_ms: None,
            pre_delay_ms: None,
            post_delay_ms: None,
            pre_wait_freezes_ms: None,
            post_wait_freezes_ms: None,
        }
    }
}

impl OperationDefaults {
    fn validate(self) -> CliOutcome<()> {
        for (name, value) in [
            ("timeout_ms", self.timeout_ms),
            ("max_attempts", self.max_attempts.map(u64::from)),
            ("retry_interval_ms", self.retry_interval_ms),
        ] {
            if value == Some(0) {
                return Err(CliError::package_invalid(format!(
                    "operation defaults {name} must be positive when provided"
                )));
            }
        }
        Ok(())
    }
}

fn default_template_threshold() -> f32 {
    DEFAULT_TEMPLATE_THRESHOLD
}

#[derive(Debug, Clone, Deserialize)]
struct OperationAnchor {
    id: String,
    template: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct Operation {
    id: String,
    purpose: String,
    from: String,
    #[serde(default)]
    to: Option<PageDeclaration>,
    click: OperationClick,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    expect_after: Option<OperationExpectation>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    pre_delay_ms: Option<u64>,
    #[serde(default)]
    post_delay_ms: Option<u64>,
    #[serde(default)]
    pre_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    post_wait_freezes_ms: Option<u64>,
    #[serde(default)]
    retryable: Option<bool>,
    #[serde(default)]
    effect: Option<String>,
    #[serde(default)]
    on_error: Option<String>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
    #[serde(default)]
    consumes: Vec<String>,
    #[serde(default)]
    produces: Vec<String>,
    #[serde(default)]
    verified_live: Option<bool>,
    #[serde(default)]
    provenance: Option<Value>,
}

impl Operation {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        for (name, value) in [("id", &self.id), ("from", &self.from)] {
            if value.trim().is_empty() {
                return Err(CliError::package_invalid(format!(
                    "operation {name} must not be empty"
                )));
            }
        }
        self.click.validate(control)?;
        if matches!(
            self.click.kind.as_str(),
            "offset" | "target" | "target_center"
        ) {
            let guard = self.guard.as_ref().ok_or_else(|| {
                CliError::package_invalid(format!(
                    "operation '{}' {} click requires guard metadata",
                    self.id, self.click.kind
                ))
            })?;
            if let Some(target_id) = self.click.target_id.as_deref()
                && target_id != guard.target_id
            {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' {} click target_id '{}' does not match guard target_id '{}'",
                    self.id, self.click.kind, target_id, guard.target_id
                )));
            }
            if guard.verify_template.is_none() {
                return Err(CliError::package_invalid(format!(
                    "operation '{}' {} click requires template guard metadata; color-probe guards cannot produce a matched_rect",
                    self.id, self.click.kind
                )));
            }
        }
        if let Some(to) = &self.to {
            to.validate(&format!("operation '{}' to", self.id))?;
        }
        if let Some(expect_after) = &self.expect_after {
            expect_after.validate(&self.id)?;
        }
        if let (Some(to), Some(expect_after)) = (&self.to, &self.expect_after)
            && !to.has_same_pages_as(&expect_after.page_id)
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' has conflicting to and expect_after.page_id declarations",
                self.id
            )));
        }
        self.validate_flow()?;
        self.validate_guard(control)
    }

    fn validate_flow(&self) -> CliOutcome<()> {
        if self.timeout_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' timeout_ms must be positive when provided",
                self.id
            )));
        }
        if self.max_attempts == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' max_attempts must be positive when provided",
                self.id
            )));
        }
        if self.retry_interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{}' retry_interval_ms must be positive when provided",
                self.id
            )));
        }
        if let Some(effect) = &self.effect
            && effect != "navigation_only"
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' effect '{effect}' is unsupported; expected navigation_only",
                self.id
            )));
        }
        if let Some(on_error) = &self.on_error
            && on_error != "return_home"
        {
            return Err(CliError::package_invalid(format!(
                "operation '{}' on_error '{on_error}' is unsupported; expected return_home",
                self.id
            )));
        }
        Ok(())
    }

    fn validate_guard(&self, control: &LabControl) -> CliOutcome<()> {
        match (&self.guard, self.unguarded_trusted_coordinate) {
            (Some(_), true) => Err(CliError::package_invalid(format!(
                "operation '{}' cannot set both guard and unguarded_trusted_coordinate",
                self.id
            ))),
            (None, true) => Ok(()),
            (None, false) => Err(CliError::package_invalid(format!(
                "operation '{}' coordinate action missing guard metadata; add guard or set unguarded_trusted_coordinate for reviewed trusted coordinates",
                self.id
            ))),
            (Some(guard), false) => guard.validate(&self.id, &self.from, control),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PageDeclaration {
    Singleton(String),
    Finite(Vec<String>),
}

impl PageDeclaration {
    fn pages(&self) -> &[String] {
        match self {
            Self::Singleton(page) => std::slice::from_ref(page),
            Self::Finite(pages) => pages,
        }
    }

    fn validate(&self, label: &str) -> CliOutcome<()> {
        let pages = self.pages();
        if pages.is_empty() {
            return Err(CliError::package_invalid(format!(
                "{label} must be a non-empty page set"
            )));
        }
        let mut unique = BTreeSet::new();
        for page in pages {
            if page.trim().is_empty() || page.trim() != page || page == "any" {
                return Err(CliError::package_invalid(format!(
                    "{label} contains invalid exact page identifier '{page}'"
                )));
            }
            if !unique.insert(page) {
                return Err(CliError::package_invalid(format!(
                    "{label} contains a duplicate page identifier '{page}'"
                )));
            }
        }
        Ok(())
    }

    fn has_same_pages_as(&self, other: &Self) -> bool {
        self.pages().iter().collect::<BTreeSet<_>>()
            == other.pages().iter().collect::<BTreeSet<_>>()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationExpectation {
    page_id: PageDeclaration,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn validate(&self, operation_id: &str) -> CliOutcome<()> {
        self.page_id
            .validate(&format!("operation '{operation_id}' expect_after.page_id"))?;
        if self.timeout_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.timeout_ms must be positive when provided"
            )));
        }
        if self.interval_ms == Some(0) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.interval_ms must be positive when provided"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: PackRect,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    color_probe: Option<String>,
}

impl OperationGuard {
    fn validate(
        &self,
        operation_id: &str,
        operation_from: &str,
        control: &LabControl,
    ) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id must not be empty"
            )));
        }
        if self.target_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.target_id must not be empty"
            )));
        }
        if !page_anchor_matches(&control.game, &self.page_id, operation_from) {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard.page_id '{}' does not match operation from '{}'",
                self.page_id, operation_from
            )));
        }
        validate_guard_rect(self.expected_rect, &control.resolution)?;
        if self.verify_template.is_none() && self.color_probe.is_none() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' guard requires verify_template or color_probe"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationClick {
    kind: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    from_rect: Option<PackRect>,
    #[serde(default)]
    to_rect: Option<PackRect>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    offset: Option<PackRect>,
    #[serde(default)]
    target_id: Option<String>,
}

impl OperationClick {
    fn validate(&self, control: &LabControl) -> CliOutcome<()> {
        match self.kind.as_str() {
            "rect" | "specific_rect" => validate_click_rect(
                self.required_rect()?,
                &control.resolution,
                control.allow_placeholder_coords.unwrap_or(false),
            ),
            "point" => validate_click_point(
                self.x
                    .ok_or_else(|| CliError::package_invalid("point click missing x"))?,
                self.y
                    .ok_or_else(|| CliError::package_invalid("point click missing y"))?,
                &control.resolution,
                control.allow_placeholder_coords.unwrap_or(false),
            ),
            "long_press" | "long_tap" => {
                let x = self
                    .x
                    .ok_or_else(|| CliError::package_invalid("long_press click missing x"))?;
                let y = self
                    .y
                    .ok_or_else(|| CliError::package_invalid("long_press click missing y"))?;
                validate_click_point(
                    x,
                    y,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "long_press duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            "offset" => {
                let offset = self
                    .offset
                    .ok_or_else(|| CliError::package_invalid("offset click missing offset rect"))?;
                if offset.width <= 0 || offset.height <= 0 {
                    return Err(CliError::package_invalid(format!(
                        "offset click dimensions must be positive: {}x{}",
                        offset.width, offset.height
                    )));
                }
                Ok(())
            }
            "target" | "target_center" => {
                if let Some(offset) = self.offset
                    && (offset.width <= 0 || offset.height <= 0)
                {
                    return Err(CliError::package_invalid(format!(
                        "target click offset dimensions must be positive: {}x{}",
                        offset.width, offset.height
                    )));
                }
                Ok(())
            }
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing from rect"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| CliError::package_invalid("drag click missing to rect"))?;
                validate_click_rect(
                    from,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                validate_click_rect(
                    to,
                    &control.resolution,
                    control.allow_placeholder_coords.unwrap_or(false),
                )?;
                if self.duration_ms.unwrap_or(0) == 0 {
                    return Err(CliError::package_invalid(
                        "drag duration_ms must be positive",
                    ));
                }
                Ok(())
            }
            other => Err(CliError::package_invalid(format!(
                "unknown operation click kind '{other}'"
            ))),
        }
    }

    fn required_rect(&self) -> CliOutcome<PackRect> {
        Ok(PackRect {
            x: self
                .x
                .ok_or_else(|| CliError::package_invalid("rect click missing x"))?,
            y: self
                .y
                .ok_or_else(|| CliError::package_invalid("rect click missing y"))?,
            width: self
                .width
                .ok_or_else(|| CliError::package_invalid("rect click missing width"))?,
            height: self
                .height
                .ok_or_else(|| CliError::package_invalid("rect click missing height"))?,
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, Deserialize)]
struct FrameStoreControl {
    #[serde(default)]
    similarity_threshold: Option<f32>,
    #[serde(default)]
    tier1_ratio: Option<f64>,
    #[serde(default)]
    tier2_ratio: Option<f64>,
    #[serde(default)]
    tier3_ratio: Option<f64>,
    #[serde(default)]
    hysteresis_ratio: Option<f64>,
    #[serde(default)]
    max_mem_bytes: Option<u64>,
    #[serde(default)]
    os_reserve_bytes: Option<u64>,
    #[serde(default)]
    flush_workspace_reserve_bytes: Option<u64>,
}

impl FrameStoreControl {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.similarity_threshold {
            validate_ratio_f32("frame_store.similarity_threshold", value)?;
        }
        for (name, value) in [
            ("frame_store.tier1_ratio", self.tier1_ratio),
            ("frame_store.tier2_ratio", self.tier2_ratio),
            ("frame_store.tier3_ratio", self.tier3_ratio),
            ("frame_store.hysteresis_ratio", self.hysteresis_ratio),
        ] {
            if let Some(value) = value {
                validate_ratio_f64(name, value)?;
            }
        }
        if self.max_mem_bytes == Some(0) {
            return Err("frame_store.max_mem_bytes must be positive when provided".to_string());
        }
        if self.flush_workspace_reserve_bytes == Some(0) {
            return Err(
                "frame_store.flush_workspace_reserve_bytes must be positive when provided"
                    .to_string(),
            );
        }
        Ok(())
    }
}

fn validate_ratio_f32(name: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value < 1.0 {
        Ok(())
    } else {
        Err(format!("{name} must be > 0 and < 1"))
    }
}

fn validate_ratio_f64(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 && value < 1.0 {
        Ok(())
    } else {
        Err(format!("{name} must be > 0 and < 1"))
    }
}

fn canonical_page_anchor(game: &str, page_id: &str) -> String {
    let prefix = format!("{game}/");
    page_id.strip_prefix(&prefix).unwrap_or(page_id).to_string()
}

fn page_anchor_matches(game: &str, observed_or_anchor: &str, expected_anchor: &str) -> bool {
    expected_anchor == "any"
        || observed_or_anchor == expected_anchor
        || canonical_page_anchor(game, observed_or_anchor) == expected_anchor
        || observed_or_anchor == format!("{game}/{expected_anchor}")
}

fn validate_click_rect(
    rect: PackRect,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "click rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_click_point(rect.x, rect.y, resolution, allow_placeholder)?;
    validate_click_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        resolution,
        allow_placeholder,
    )?;
    if !allow_placeholder
        && rect.x == 0
        && rect.y == 0
        && rect.width as u32 == resolution.width
        && rect.height as u32 == resolution.height
    {
        return Err(CliError::package_invalid(
            "full-screen click rect is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn validate_guard_rect(rect: PackRect, resolution: &Resolution) -> CliOutcome<()> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(CliError::package_invalid(format!(
            "guard expected_rect dimensions must be positive: {}x{}",
            rect.width, rect.height
        )));
    }
    validate_rect_point(rect.x, rect.y, resolution, "guard expected_rect")?;
    validate_rect_point(
        rect.x + rect.width - 1,
        rect.y + rect.height - 1,
        resolution,
        "guard expected_rect",
    )
}

fn validate_rect_point(x: i32, y: i32, resolution: &Resolution, label: &str) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "{label} point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
    }
    Ok(())
}

fn validate_click_point(
    x: i32,
    y: i32,
    resolution: &Resolution,
    allow_placeholder: bool,
) -> CliOutcome<()> {
    if x < 0 || y < 0 || x >= resolution.width as i32 || y >= resolution.height as i32 {
        return Err(CliError::package_invalid(format!(
            "click point {x},{y} is outside {}x{}",
            resolution.width, resolution.height
        )));
    }
    if !allow_placeholder && x == 0 && y == 0 {
        return Err(CliError::package_invalid(
            "click point 0,0 is treated as unresolved coordinates",
        ));
    }
    Ok(())
}

fn containment_error(error: ContainmentError) -> CliError {
    CliError::package_invalid(error.to_string())
}

fn resolve_package_resource_root(input: &Path) -> CliOutcome<ResolvedResourceRoot> {
    require_approved_directory(input, input)?;
    let (root, layout) = if looks_like_approved_resource_root(input)? {
        (input.to_path_buf(), "direct")
    } else {
        let ours = input.join("ours");
        if looks_like_approved_resource_root(&ours)? {
            (ours, "repo_ours")
        } else {
            (input.to_path_buf(), "unresolved")
        }
    };
    validate_resource_tree_boundary(&root)?;
    Ok(ResolvedResourceRoot {
        input: input.to_path_buf(),
        root,
        layout,
    })
}

fn looks_like_approved_resource_root(path: &Path) -> CliOutcome<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(CliError::package_invalid(format!(
                "failed to inspect package source path {}: {err}",
                path.display()
            )));
        }
    };
    reject_link_or_reparse(path, &metadata)?;
    if !metadata.is_dir() {
        return Ok(false);
    }
    Ok(approved_directory_exists(path, &path.join("operations"))?
        && (approved_directory_exists(path, &path.join("recognition"))?
            || approved_directory_exists(path, &path.join("navigation"))?))
}

fn validate_resource_tree_boundary(root: &Path) -> CliOutcome<()> {
    require_approved_directory(root, root)?;
    let canonical_root = canonical_approved_root(root)?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to inspect approved resource directory {}: {err}",
                    directory.display()
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to inspect approved resource directory {}: {err}",
                    directory.display()
                ))
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = link_safe_metadata(&path)?;
            ensure_canonical_containment(&canonical_root, &path)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() {
                return Err(CliError::package_invalid(format!(
                    "package source path {} is neither a regular file nor a directory",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn read_approved_file(approved_root: &Path, path: &Path) -> CliOutcome<Vec<u8>> {
    let metadata = inspect_approved_path(approved_root, path)?.ok_or_else(|| {
        CliError::package_invalid(format!(
            "approved package source file does not exist: {}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CliError::package_invalid(format!(
            "approved package source path is not a regular file: {}",
            path.display()
        )));
    }
    fs::read(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })
}

fn approved_file_exists(approved_root: &Path, path: &Path) -> CliOutcome<bool> {
    let Some(metadata) = inspect_approved_path(approved_root, path)? else {
        return Ok(false);
    };
    if !metadata.is_file() {
        return Err(CliError::package_invalid(format!(
            "approved package source path is not a regular file: {}",
            path.display()
        )));
    }
    Ok(true)
}

fn approved_directory_exists(approved_root: &Path, path: &Path) -> CliOutcome<bool> {
    let Some(metadata) = inspect_approved_path(approved_root, path)? else {
        return Ok(false);
    };
    if !metadata.is_dir() {
        return Ok(false);
    }
    Ok(true)
}

fn require_approved_directory(approved_root: &Path, path: &Path) -> CliOutcome<()> {
    let metadata = inspect_approved_path(approved_root, path)?.ok_or_else(|| {
        CliError::package_invalid(format!(
            "approved resource directory does not exist: {}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(CliError::package_invalid(format!(
            "approved resource path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn inspect_approved_path(approved_root: &Path, path: &Path) -> CliOutcome<Option<fs::Metadata>> {
    let root_metadata = link_safe_metadata(approved_root)?;
    if !root_metadata.is_dir() {
        return Err(CliError::package_invalid(format!(
            "approved resource root is not a directory: {}",
            approved_root.display()
        )));
    }
    let relative = path.strip_prefix(approved_root).map_err(|_| {
        CliError::package_invalid(format!(
            "package source path {} is outside approved resource root {}",
            path.display(),
            approved_root.display()
        ))
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CliError::package_invalid(format!(
            "package source path {} escapes approved resource root {}",
            path.display(),
            approved_root.display()
        )));
    }

    let canonical_root = canonical_approved_root(approved_root)?;
    if relative.as_os_str().is_empty() {
        return Ok(Some(root_metadata));
    }
    let components = relative.components().collect::<Vec<_>>();
    let mut current = approved_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::CurDir => continue,
            Component::Normal(value) => current.push(value),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => unreachable!(),
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CliError::package_invalid(format!(
                    "failed to inspect package source path {}: {err}",
                    current.display()
                )));
            }
        };
        reject_link_or_reparse(&current, &metadata)?;
        ensure_canonical_containment(&canonical_root, &current)?;
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(CliError::package_invalid(format!(
                "package source parent is not a directory: {}",
                current.display()
            )));
        }
        if index + 1 == components.len() {
            return Ok(Some(metadata));
        }
    }
    Ok(None)
}

fn link_safe_metadata(path: &Path) -> CliOutcome<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to inspect package source path {}: {err}",
            path.display()
        ))
    })?;
    reject_link_or_reparse(path, &metadata)?;
    Ok(metadata)
}

fn reject_link_or_reparse(path: &Path, metadata: &fs::Metadata) -> CliOutcome<()> {
    if metadata.file_type().is_symlink() || metadata_is_reparse_point(metadata) {
        return Err(CliError::package_invalid(format!(
            "package source path {} is a symlink or reparse point and cannot be read",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn canonical_approved_root(root: &Path) -> CliOutcome<PathBuf> {
    fs::canonicalize(root).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to canonicalize approved resource root {}: {err}",
            root.display()
        ))
    })
}

fn ensure_canonical_containment(canonical_root: &Path, path: &Path) -> CliOutcome<()> {
    let canonical = fs::canonicalize(path).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to canonicalize package source path {}: {err}",
            path.display()
        ))
    })?;
    if !canonical.starts_with(canonical_root) {
        return Err(CliError::package_invalid(format!(
            "package source path {} resolves outside approved resource root {}",
            path.display(),
            canonical_root.display()
        )));
    }
    Ok(())
}

struct ResolvedRepo {
    path: PathBuf,
    remote_url: Option<String>,
    temp_root: Option<PathBuf>,
}

impl ResolvedRepo {
    fn from_source(source: PackageSource, temporary_root: &Path) -> CliOutcome<Self> {
        match source {
            PackageSource::Remote(url) => Self::clone_remote(url, temporary_root),
            PackageSource::Local(path) => Ok(Self {
                path,
                remote_url: None,
                temp_root: None,
            }),
        }
    }

    fn clone_remote(url: String, temporary_root: &Path) -> CliOutcome<Self> {
        let root = temporary_root.join(format!(
            "actinglab-resource-remote-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let path = root.join("repo");
        fs::create_dir_all(&root).map_err(|err| {
            CliError::usage(format!("failed to create {}: {err}", root.display()))
        })?;
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url])
            .arg(&path)
            .output()
            .map_err(|err| CliError::usage(format!("failed to start git clone: {err}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let primary = CliError::usage(format!("git clone failed: {}", stderr.trim()));
            return Err(match fs::remove_dir_all(&root) {
                Ok(()) => primary,
                Err(error) => combine_package_errors(
                    primary,
                    CliError::package_invalid(format!(
                        "failed to remove incomplete remote clone {}: {error}",
                        root.display()
                    )),
                ),
            });
        }
        Ok(Self {
            path,
            remote_url: Some(url),
            temp_root: Some(root),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn remote_url(&self) -> Option<String> {
        self.remote_url.clone()
    }

    fn cleanup(&mut self) -> CliOutcome<()> {
        if let Some(root) = self.temp_root.take() {
            fs::remove_dir_all(&root).map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to remove remote temp directory {}: {err}",
                    root.display()
                ))
            })?;
        }
        Ok(())
    }
}

impl Drop for ResolvedRepo {
    fn drop(&mut self) {
        if let Some(root) = self.temp_root.take()
            && let Err(error) = fs::remove_dir_all(&root)
        {
            eprintln!(
                "WARNING package source cleanup failed during drop; path={}; original_error={error}; fallback=retain_temp_directory; impact=disk_space",
                root.display()
            );
        }
    }
}

fn read_json_value(approved_root: &Path, path: &Path) -> CliOutcome<Value> {
    let bytes = read_approved_file(approved_root, path)?;
    let text = std::str::from_utf8(&bytes).map_err(|err| {
        CliError::package_invalid(format!("{} is not valid UTF-8: {err}", path.display()))
    })?;
    serde_json::from_str(text).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {}: {err}", path.display()))
    })
}

fn collect_files(approved_root: &Path, root: &Path) -> CliOutcome<Vec<PathBuf>> {
    require_approved_directory(approved_root, root)?;
    let mut files = Vec::new();
    collect_files_inner(approved_root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_inner(
    approved_root: &Path,
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> CliOutcome<()> {
    let mut entries = fs::read_dir(root)
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", root.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", root.display()))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = inspect_approved_path(approved_root, &path)?.ok_or_else(|| {
            CliError::package_invalid(format!(
                "package source path disappeared during collection: {}",
                path.display()
            ))
        })?;
        if metadata.is_dir() {
            collect_files_inner(approved_root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        } else {
            return Err(CliError::package_invalid(format!(
                "package source path {} is neither a regular file nor a directory",
                path.display()
            )));
        }
    }
    Ok(())
}

fn relative_slash(root: &Path, path: &Path) -> CliOutcome<String> {
    let rel = path.strip_prefix(root).map_err(|err| {
        CliError::package_invalid(format!(
            "{} is outside {}: {err}",
            path.display(),
            root.display()
        ))
    })?;
    Ok(rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn validate_zip_entry_path(path: &str) -> CliOutcome<()> {
    if path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::package_invalid(format!(
            "unsafe package entry path: {path}"
        )));
    }
    if DANGEROUS_EXTENSIONS.iter().any(|extension| {
        Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    }) {
        return Err(CliError::package_invalid(format!(
            "package entry has dangerous extension: {path}"
        )));
    }
    Ok(())
}

fn is_safe_package_relative_path(path: &str) -> bool {
    !path.ends_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.starts_with('/')
        && !Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn task_verify_template_paths(task: &Value) -> Vec<&str> {
    let mut paths = Vec::new();
    collect_task_verify_template_paths(task, &mut paths);
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn collect_task_verify_template_paths<'a>(value: &'a Value, paths: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            if let Some(path) = object.get("verify_template").and_then(Value::as_str)
                && !path.trim().is_empty()
            {
                paths.push(path);
            }
            for value in object.values() {
                collect_task_verify_template_paths(value, paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_task_verify_template_paths(value, paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn temp_zip_path(out: &Path) -> CliOutcome<PathBuf> {
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    let file_name = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("actinglab-package.zip");
    Ok(parent.join(format!(
        ".{file_name}.tmp-{}-{}.zip",
        std::process::id(),
        unique_suffix()
    )))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn json_u32(value: &Value, key: &str) -> CliOutcome<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CliError::package_invalid(format!("missing u32 field {key}")))
}

fn array_field<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn ordered_object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn validate_capture_backend(value: &str) -> CliOutcome<()> {
    if matches!(
        value,
        "auto"
            | "auto-fastest"
            | "auto_fastest"
            | "adb"
            | "adb_screencap"
            | "screencap"
            | "droidcast_raw"
            | "droidcast"
            | "nemu_ipc"
            | "nemu"
    ) {
        Ok(())
    } else {
        Err(CliError::package_invalid(format!(
            "unknown capture backend '{value}', expected auto, auto-fastest, adb, droidcast_raw, or nemu_ipc"
        )))
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn zip_write_error(error: zip::result::ZipError) -> CliError {
    CliError::package_invalid(format!("zip write failed: {error}"))
}

fn zip_io_error(error: io::Error) -> CliError {
    CliError::package_invalid(format!("zip I/O failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageEnvOptions;
    use std::io::Read;
    use tempfile::TempDir;
    use zip::ZipArchive;

    #[test]
    fn packaged_drag_validation_accepts_only_canonical_endpoints() {
        let control: LabControl = serde_json::from_value(
            control_json(
                "neutral.test.fixture",
                "navigable_route",
                "neutral",
                "test",
                (100, 100),
                "fixture",
                None,
            )
            .expect("control JSON"),
        )
        .expect("control");
        let canonical: OperationClick = serde_json::from_value(json!({
            "kind": "drag",
            "from_rect": {"x": 1, "y": 2, "width": 10, "height": 11},
            "to_rect": {"x": 50, "y": 60, "width": 12, "height": 13},
            "duration_ms": 250
        }))
        .expect("canonical drag");
        canonical
            .validate(&control)
            .expect("canonical drag validates");

        let legacy: OperationClick = serde_json::from_value(json!({
            "kind": "drag",
            "from": {"x": 1, "y": 2, "width": 10, "height": 11},
            "to": {"x": 50, "y": 60, "width": 12, "height": 13},
            "duration_ms": 250
        }))
        .expect("legacy-shaped drag parses without aliases");
        let error = legacy
            .validate(&control)
            .expect_err("packaged source endpoint names must fail");
        assert!(error.message.contains("drag click missing from rect"));
    }

    #[test]
    fn build_task_accepts_source_drag_after_converter_canonicalization() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |task| {
            task["operations"][0]["click"] = json!({
                "kind": "drag",
                "from": {"x": 100, "y": 100, "width": 20, "height": 20},
                "to": {"x": 200, "y": 100, "width": 20, "height": 20},
                "duration_ms": 250
            });
        });
        let out = temp.path().join("canonical-drag.zip");

        build_task(build_task_request(repo, out.clone()))
            .expect("official package build accepts converter-normalized drag");

        assert_published_file(&out);
    }

    #[test]
    fn generated_package_closes_hash_bound_post_admission_ocr_truth_entry() {
        let temp = TempDir::new().expect("temp");
        let approved = temp.path().join("approved");
        fs::create_dir_all(&approved).expect("approved root");
        let truth = serde_json::to_vec(&json!({
            "schema_version": "actingcommand.ocr-truth-set.v1",
            "items": ["Alpha", "Beta"]
        }))
        .expect("truth bytes");
        let truth_path = approved.join("truth.json");
        fs::write(&truth_path, &truth).expect("truth file");
        let truth_sha256 = hex_sha256(&truth);
        let control: LabControl =
            serde_json::from_value(stability_control_fixture("navigable_route", None, None))
                .expect("control");
        let mut operation = stability_operation_fixture(None);
        operation["schema_version"] = json!("0.7");
        operation["scheduling_outcome"] = json!({
            "mappings": [{
                "outcome_key": "comparison_recorded",
                "effect": "no_designated_effect",
                "terminal_pages": ["terminal"]
            }]
        });
        operation["post_admission_ocr"] = json!({
            "page_id": "terminal",
            "target_id": "fixture/ocr",
            "truth_set": {"path": "truth.json", "sha256": truth_sha256},
            "normalization": "trim_lowercase_v1",
            "comparison": "exact_set_v1",
            "limits": {
                "max_frames": 2,
                "max_items": 16,
                "max_string_bytes": 64,
                "max_total_bytes": 4096,
                "max_truth_entries": 16
            },
            "outcome_key": "comparison_recorded"
        });
        let operation: OperationBundle =
            serde_json::from_value(operation).expect("operation bundle");
        operation
            .validate(&control, |_| Ok(true))
            .expect("declaration validation");
        let mut entries = PackageEntries::new(&approved, 4096).expect("entries");
        let entry_path = format!("resources/operations/{}/truth.json", control.entry_task_id);
        entries
            .add_file(&truth_path, &entry_path)
            .expect("truth package entry");
        let archive_path = temp.path().join("fixture.zip");
        write_zip(&archive_path, &mut entries).expect("fixture archive");

        validate_generated_post_admission_ocr(&archive_path, &entries, &control, &operation)
            .expect("generated truth closure");

        let mut mismatch = operation.clone();
        mismatch
            .post_admission_ocr
            .as_mut()
            .expect("declaration")
            .truth_set
            .sha256 = "0".repeat(64);
        assert!(
            validate_generated_post_admission_ocr(&archive_path, &entries, &control, &mismatch,)
                .expect_err("hash mismatch")
                .message
                .contains("hash mismatch")
        );
    }

    fn build_task(request: PackageBuildTaskRequest) -> CliOutcome<PackageBuildTaskResponse> {
        prepare_package_build_task(request)?.build(&AuthoringEnvironmentSnapshot::default())
    }

    fn build_task_request(repo: PathBuf, out: PathBuf) -> PackageBuildTaskRequest {
        PackageBuildTaskRequest {
            source: PackageSource::Local(repo),
            temporary_root: out.parent().unwrap().join("remote-source"),
            task_id: "operator_task".to_string(),
            game: None,
            server: None,
            locale: None,
            package_id: None,
            execution_mode: None,
            resolution: None,
            include_recovery: false,
            out,
            dry_run: false,
            max_buffered_payload_bytes: crate::DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
            env: PackageEnvOptions::default(),
        }
    }

    fn build_catalog_request(repo: PathBuf, temporary_root: PathBuf) -> PackageBuildCatalogRequest {
        PackageBuildCatalogRequest {
            source: PackageSource::Local(repo),
            temporary_root,
            game: None,
            server: None,
            locale: None,
            max_buffered_payload_bytes: crate::DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        }
    }

    #[test]
    fn package_read_rejects_path_outside_approved_root() {
        let temp = TempDir::new().expect("temp");
        let approved = temp.path().join("approved");
        fs::create_dir_all(&approved).expect("approved root");
        let outside = temp.path().join("outside.png");
        fs::write(&outside, b"outside").expect("outside file");

        let error = read_approved_file(&approved, &outside)
            .expect_err("root-external source must fail before reading");

        assert!(error.message.contains(&outside.display().to_string()));
        assert!(error.message.contains("outside approved resource root"));
    }

    #[test]
    fn package_entries_reject_single_source_over_budget_before_streaming() {
        let temp = TempDir::new().expect("temp");
        let approved = temp.path().join("approved");
        fs::create_dir_all(&approved).expect("approved root");
        let source = approved.join("large.bin");
        let budget = 1024usize;
        File::create(&source)
            .expect("large source")
            .set_len((budget + 1) as u64)
            .expect("large source length");
        let mut entries = PackageEntries::new(&approved, budget).expect("entries");

        let error = entries
            .add_file(&source, "resources/large.bin")
            .expect_err("entry larger than the budget must fail before streaming");

        assert!(error.message.contains(&source.display().to_string()));
        assert!(error.message.contains("max_buffered_payload_bytes=1024"));
        assert_eq!(entries.stats, PackagePayloadStats::default());
        assert!(entries.files.is_empty());
    }

    #[test]
    fn generated_payload_stops_at_aggregate_buffer_budget() {
        let temp = TempDir::new().expect("temp");
        let approved = temp.path().join("approved");
        fs::create_dir_all(&approved).expect("approved root");
        let budget = 128usize;
        let mut entries = PackageEntries::new(&approved, budget).expect("entries");

        let error = entries
            .add_json("large.json", json!({"value": "x".repeat(512)}))
            .expect_err("generated payload must stop at the aggregate budget");

        assert!(error.message.contains("max_buffered_payload_bytes=128"));
        assert!(entries.stats.buffered_payload_bytes <= budget);
        assert!(entries.files.is_empty());
    }

    #[test]
    fn production_validation_peak_is_bounded_when_total_exceeds_four_budgets() {
        let temp = TempDir::new().expect("temp");
        let approved = temp.path().join("approved");
        fs::create_dir_all(&approved).expect("approved root");
        let budget = 128 * 1024usize;
        let source_size = 64 * 1024usize;
        let source_count = 8usize;
        let mut entries = PackageEntries::new(&approved, budget).expect("entries");

        for index in 0..source_count {
            let source = approved.join(format!("asset-{index}.bin"));
            fs::write(&source, vec![index as u8; source_size]).expect("source asset");
            entries
                .add_file(&source, &format!("resources/assets/asset-{index}.bin"))
                .expect("streamed source");
        }
        entries
            .add_json(
                "control.json",
                control_json(
                    "arknights.cn.operator_task",
                    "navigable_route",
                    "arknights",
                    "cn",
                    (1280, 720),
                    "operator_task",
                    None,
                )
                .expect("control"),
            )
            .expect("control");
        entries
            .add_json(
                "resources/operations/operator_task/task.json",
                json!({
                    "schema_version": "0.3",
                    "task_id": "operator_task",
                    "game": "arknights",
                    "server_scope": ["cn"],
                    "coordinate_space": {"width": 1280, "height": 720},
                    "operations": [{
                        "id": "noop",
                        "purpose": "bounded validation fixture",
                        "from": "home",
                        "to": null,
                        "click": {"kind": "point", "x": 10, "y": 10},
                        "unguarded_trusted_coordinate": true
                    }]
                }),
            )
            .expect("operation");
        entries
            .add_json(
                "resources/recognition/arknights.cn.pack.json",
                json!({
                    "schema_version": "0.3",
                    "game": "arknights",
                    "server": "cn",
                    "coordinate_space": {"width": 1280, "height": 720},
                    "targets": [{
                        "type": "color",
                        "id": "page/home",
                        "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                        "expected": [0, 0, 0]
                    }]
                }),
            )
            .expect("recognition pack");
        entries
            .add_json(
                "resources/recognition/arknights.cn.pages.json",
                json!({
                    "schema_version": "0.3",
                    "pages": [{"id": "home", "required": ["page/home"]}]
                }),
            )
            .expect("pages");
        entries
            .add_json(
                "resources/navigation/arknights.cn.navigation.json",
                json!({"schema_version": "0.3", "control_points": []}),
            )
            .expect("navigation");
        entries.add_manifest("operator_task").expect("manifest");
        let out = temp.path().join("bounded.zip");
        let write = write_and_validate_package(&out, entries, false).expect("bounded package");

        let logical_source_bytes = source_size * source_count;
        assert!(logical_source_bytes >= budget * 4);
        assert!(
            write.stats.logical_payload_bytes
                >= u64::try_from(logical_source_bytes).expect("logical bytes")
        );
        assert!(write.stats.peak_buffered_payload_bytes <= budget);
        assert!(write.stats.peak_streamed_payload_bytes <= PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES);
        assert!(
            write.stats.peak_resident_payload_bytes
                <= budget + PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES
        );
        assert_eq!(
            write.stats.validation_streamed_payload_bytes,
            write.stats.logical_payload_bytes
        );
        assert!(
            write.stats.peak_validation_streamed_payload_bytes
                <= PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES
        );
        assert!(
            write.stats.peak_validation_resident_payload_bytes
                <= budget + PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES
        );
        assert_eq!(write.validation.status, "valid");
        assert_published_file(&out);
    }

    #[cfg(unix)]
    #[test]
    fn build_task_rejects_symlinked_asset_before_read() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let outside = temp.path().join("outside.png");
        fs::write(&outside, one_pixel_png()).expect("outside asset");
        let linked = repo.join("operations/operator_task/assets/LINKED.png");
        symlink(&outside, &linked).expect("asset symlink");

        let error = build_task(build_task_request(
            repo,
            temp.path().join("symlink-rejected.zip"),
        ))
        .expect_err("symlinked source must fail before reading");

        assert!(error.message.contains(&linked.display().to_string()));
        assert!(error.message.contains("symlink or reparse point"));
    }

    #[cfg(windows)]
    #[test]
    fn build_task_rejects_junction_reparse_point_before_read() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let outside = temp.path().join("outside-assets");
        fs::create_dir_all(&outside).expect("outside directory");
        fs::write(outside.join("OUTSIDE.png"), one_pixel_png()).expect("outside asset");
        let junction = repo
            .join("operations")
            .join("operator_task")
            .join("junction-assets");
        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .expect("create junction");
        assert!(
            output.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let error = build_task(build_task_request(
            repo,
            temp.path().join("junction-rejected.zip"),
        ))
        .expect_err("junction source must fail before reading");

        assert!(error.message.contains(&junction.display().to_string()));
        assert!(error.message.contains("symlink or reparse point"));
    }

    #[test]
    fn build_task_package_validates_and_rewrites_template_paths() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let out = temp.path().join("task.zip");

        build_task(build_task_request(repo, out.clone())).unwrap();
        assert_published_file(&out);
        let entries = read_zip_entries(&out);
        assert!(entries.contains_key("control.json"));
        assert!(entries.contains_key("resources/manifest.json"));
        assert!(entries.contains_key("resources/operations/operator_task/task.json"));
        assert!(
            entries.contains_key("resources/operations/operator_task/assets/PAGE_OPERATOR_0.png")
        );
        assert!(
            entries.contains_key("resources/operations/operator_task/assets/PAGE_OPERATOR_1.png")
        );
        let pack: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pack.json")
                .unwrap(),
        )
        .unwrap();
        let paths = pack["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target["template_path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        for path in paths {
            assert!(entries.contains_key(&format!("resources/{path}")), "{path}");
        }
        let pages: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pages.json")
                .unwrap(),
        )
        .unwrap();
        let operator = pages["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|page| page["id"] == "arknights/operator")
            .unwrap();
        assert_eq!(operator["required"], json!([]));
        assert_eq!(
            operator["any_of"],
            json!([["page/operator_0", "page/operator_1"]])
        );
    }

    #[test]
    fn build_task_package_consumes_canonical_ocr_converter_output() {
        let ocr_id = "ocr/synthetic-label";
        let expected_text = "synthetic text";
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |task| {
            task["schema_version"] = json!("0.6");
            task["ocr_targets"] = json!([{
                "id": ocr_id,
                "region": {
                    "mode": "rect",
                    "rect": {"x": 20, "y": 30, "width": 200, "height": 40}
                },
                "languages": ["en"],
                "timeout_ms": 5_000,
                "match_mode": "contains",
                "expected": [expected_text],
                "case_sensitive": false,
                "minimum_confidence": 0.8,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }]);
            let entry_page = task["entry_page"]
                .as_str()
                .expect("fixture entry page")
                .to_string();
            let required_target = task["operations"][0]["guard"]["target_id"]
                .as_str()
                .expect("fixture guard target")
                .to_string();
            task["page_rules"] = Value::Object(Map::from_iter([(
                entry_page,
                json!({"required": [required_target], "optional": [ocr_id]}),
            )]));
        });
        let out = temp.path().join("ocr-task.zip");

        let response = build_task(build_task_request(repo, out.clone()))
            .expect("official package build and validation");

        assert_eq!(
            response
                .validation
                .resources
                .recognition_unsupported_target_count,
            0
        );
        let inspected = crate::package_validate::validate_package(crate::PackageValidateRequest {
            zip_path: out.clone(),
            include_entries: true,
            expected_input_sha256: None,
        })
        .expect("official package validate/inspect path");
        assert_eq!(inspected.status, "valid");
        assert_eq!(inspected.recognition_pack_diagnostics.len(), 1);
        let pack_path = &inspected.recognition_pack_diagnostics[0].path;
        assert_eq!(
            inspected.recognition_pack_diagnostics[0].unsupported_target_count,
            0
        );
        assert!(
            inspected
                .entries
                .as_ref()
                .unwrap()
                .iter()
                .any(|entry| entry == pack_path)
        );
        let entries = read_zip_entries(&out);
        let pack: Value =
            serde_json::from_slice(entries.get(pack_path).expect("generated recognition pack"))
                .expect("recognition pack JSON");
        let target = pack["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["id"] == ocr_id)
            .expect("canonical OCR target");
        assert_eq!(
            target,
            &json!({
                "type": "ocr",
                "id": ocr_id,
                "region": {"x":20,"y":30,"width":200,"height":40},
                "languages": ["en"],
                "timeout_ms": 5_000,
                "match_mode": "contains",
                "expected": [expected_text],
                "case_sensitive": false,
                "minimum_confidence": 0.8,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            })
        );
    }

    #[test]
    fn build_task_preserves_the_frozen_finite_page_set_shape() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |task| {
            task["target_page"] = json!(["middle", "mall"]);
            task["operations"][0]["to"] = json!(["middle", "mall"]);
            task["operations"][0]["expect_after"] = json!({
                "page_id": ["mall", "middle"],
                "timeout_ms": 500
            });
        });
        let out = temp.path().join("finite-page-set.zip");

        build_task(build_task_request(repo, out.clone()))
            .expect("the approved finite page-set shape must build");

        let entries = read_zip_entries(&out);
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("packaged operation task"),
        )
        .expect("packaged task JSON");
        assert_eq!(task["target_page"], json!(["mall", "middle"]));
        assert_eq!(task["operations"][0]["to"], json!(["mall", "middle"]));
        assert_eq!(
            task["operations"][0]["expect_after"]["page_id"],
            json!(["mall", "middle"])
        );
    }

    #[test]
    fn build_task_preserves_singleton_page_declarations() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |task| {
            task["operations"][0]["expect_after"] = json!({
                "page_id": "middle",
                "timeout_ms": 500
            });
        });
        let out = temp.path().join("singleton-page-declarations.zip");

        build_task(build_task_request(repo, out.clone()))
            .expect("existing singleton declarations must remain compatible");

        let entries = read_zip_entries(&out);
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("packaged operation task"),
        )
        .expect("packaged task JSON");
        assert_eq!(task["target_page"], "mall");
        assert_eq!(task["operations"][0]["to"], "middle");
        assert_eq!(task["operations"][0]["expect_after"]["page_id"], "middle");
    }

    #[test]
    fn build_task_rejects_invalid_page_declarations_for_every_supported_field() {
        let cases = [
            ("target-empty-set", "target_page", json!([]), "non-empty"),
            (
                "target-empty-id",
                "target_page",
                json!(""),
                "invalid exact page identifier",
            ),
            (
                "target-duplicate",
                "target_page",
                json!(["mall", "mall"]),
                "duplicate",
            ),
            (
                "target-malformed",
                "target_page",
                json!(["mall", 7]),
                "must be a string page identifier",
            ),
            ("to-empty-set", "to", json!([]), "non-empty"),
            (
                "to-empty-id",
                "to",
                json!(""),
                "invalid exact page identifier",
            ),
            (
                "to-duplicate",
                "to",
                json!(["middle", "middle"]),
                "duplicate",
            ),
            (
                "to-malformed",
                "to",
                json!(["middle", 7]),
                "must be a string page identifier",
            ),
            (
                "expect-after-empty-set",
                "expect_after.page_id",
                json!([]),
                "non-empty",
            ),
            (
                "expect-after-empty-id",
                "expect_after.page_id",
                json!(""),
                "invalid exact page identifier",
            ),
            (
                "expect-after-duplicate",
                "expect_after.page_id",
                json!(["middle", "middle"]),
                "duplicate",
            ),
            (
                "expect-after-malformed",
                "expect_after.page_id",
                json!(["middle", 7]),
                "must be a string page identifier",
            ),
        ];

        for (case, field, declaration, expected) in cases {
            let temp = TempDir::new().expect("temp");
            let repo = temp.path().join("repo");
            write_fixture_repo(&repo);
            let operation_path = repo.join("operations/operator_task/task.json");
            let mut task: Value =
                serde_json::from_slice(&fs::read(&operation_path).expect("fixture task"))
                    .expect("fixture task JSON");
            set_fixture_page_declaration(&mut task, field, declaration);

            let validator_error = validate_packaged_operation_fixture(task.clone())
                .expect_err("generated-package validation must fail closed");
            assert_eq!(validator_error.code, "package_invalid", "{case}");

            fs::write(
                &operation_path,
                serde_json::to_string_pretty(&task).expect("updated fixture JSON"),
            )
            .expect("updated fixture task");
            let out = temp.path().join(format!("{case}.zip"));
            let error = build_task(build_task_request(repo, out.clone()))
                .expect_err("invalid declaration must fail the official build path");
            assert_eq!(error.code, "package_invalid", "{case}: {error:?}");
            assert!(
                error.message.contains(expected),
                "{case}: expected '{expected}' in '{}'",
                error.message
            );
            assert!(!out.exists(), "{case}: invalid build published a ZIP");
        }
    }

    #[test]
    fn build_task_rejects_conflicting_to_and_expect_after_page_sets() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let operation_path = repo.join("operations/operator_task/task.json");
        let mut task: Value =
            serde_json::from_slice(&fs::read(&operation_path).expect("fixture task"))
                .expect("fixture task JSON");
        task["operations"][0]["to"] = json!(["middle", "mall"]);
        task["operations"][0]["expect_after"] = json!({"page_id": ["middle"]});

        let validator_error = validate_packaged_operation_fixture(task.clone())
            .expect_err("generated-package validation must reject conflicting declarations");
        assert_eq!(validator_error.code, "package_invalid");
        assert!(validator_error.message.contains("conflicting"));

        fs::write(
            &operation_path,
            serde_json::to_string_pretty(&task).expect("updated fixture JSON"),
        )
        .expect("updated fixture task");
        let out = temp.path().join("conflicting-page-sets.zip");
        let error = build_task(build_task_request(repo, out.clone()))
            .expect_err("conflicting declarations must fail the official build path");
        assert_eq!(error.code, "package_invalid");
        assert!(error.message.contains("conflicting"));
        assert!(!out.exists());
    }

    #[test]
    fn build_task_rejects_zero_max_task_retries() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |operation| {
            operation
                .as_object_mut()
                .unwrap()
                .insert("max_task_retries".to_string(), json!(0));
        });

        let error = build_task(build_task_request(
            repo,
            temp.path().join("invalid-retries.zip"),
        ))
        .expect_err("zero max_task_retries must fail package validation");

        assert!(error.message.contains("max_task_retries must be positive"));
    }

    #[test]
    fn build_task_rejects_duplicate_operation_ids() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |operation| {
            let operations = operation["operations"].as_array_mut().unwrap();
            operations[1]["id"] = operations[0]["id"].clone();
        });

        let error = build_task(build_task_request(
            repo,
            temp.path().join("duplicate-operations.zip"),
        ))
        .expect_err("duplicate operation ids must fail package validation");

        assert!(error.message.contains("duplicate operation id"));
    }

    #[test]
    fn build_task_accepts_reorganized_repo_root_with_ours_layout() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo.join("ours"));
        let out = temp.path().join("task.zip");

        let response = build_task(build_task_request(repo, out.clone())).unwrap();

        assert_eq!(response.resource_layout, "repo_ours");
        assert!(
            response.resource_root.ends_with("ours"),
            "{}",
            response.resource_root
        );
        assert_published_file(&out);
    }

    #[test]
    fn build_task_with_recovery_keeps_recovery_recognition_context() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        add_recruit_fixture(&repo);
        add_return_home_recruit_rule(&repo);
        let out = temp.path().join("task.zip");

        let mut request = build_task_request(repo, out.clone());
        request.include_recovery = true;
        build_task(request).unwrap();
        let entries = read_zip_entries(&out);
        assert!(entries.contains_key("resources/operations/recruit_task/assets/RECRUIT.png"));
        assert!(!entries.contains_key("resources/operations/recruit_task/task.json"));
        let pages: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pages.json")
                .unwrap(),
        )
        .unwrap();
        assert!(
            pages["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|page| page["id"] == "arknights/recruit")
        );
        let pack: Value = serde_json::from_slice(
            entries
                .get("resources/recognition/arknights.cn.pack.json")
                .unwrap(),
        )
        .unwrap();
        assert!(
            pack["targets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|target| target["id"] == "page/recruit")
        );
    }

    #[test]
    fn prepared_build_requires_and_applies_typed_environment_snapshot() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |operation| {
            operation["goal"] = json!("operator-{env:server}");
        });

        let missing = prepare_package_build_task(build_task_request(
            repo.clone(),
            temp.path().join("missing.zip"),
        ))
        .expect("prepare missing snapshot");
        assert_eq!(
            missing.required_environment_keys().expect("required keys"),
            vec!["server".to_string()]
        );
        assert!(
            missing
                .build(&AuthoringEnvironmentSnapshot::default())
                .is_err()
        );

        let out = temp.path().join("resolved.zip");
        let prepared = prepare_package_build_task(build_task_request(repo, out.clone()))
            .expect("prepare resolved snapshot");
        let environment = environment_snapshot("server", "cn");
        prepared.build(&environment).expect("resolved build");

        let entries = read_zip_entries(&out);
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("operation task"),
        )
        .expect("task json");
        assert_eq!(task["goal"], "operator-cn");
        let index: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operations.index.json")
                .expect("operation index"),
        )
        .expect("index json");
        let operator = index["operations"]
            .as_array()
            .expect("index operations")
            .iter()
            .find(|entry| entry["task_id"] == "operator_task")
            .expect("operator index");
        assert_eq!(operator["goal"], "operator-cn");
    }

    #[test]
    fn build_catalog_full_archive_validates() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |operation| {
            operation["goal"] = json!("operator-{env:server}");
        });
        let out = temp.path().join("full.zip");
        let catalog = PackageBuildCatalog::open(build_catalog_request(
            repo,
            temp.path().join("remote-source"),
        ))
        .unwrap();
        assert_eq!(
            catalog.full_environment_keys().expect("environment keys"),
            vec!["server".to_string()]
        );

        catalog
            .build_full_archive(
                &environment_snapshot("server", "cn"),
                PackageFullArchiveRequest {
                    entry_task_id: "operator_task".to_string(),
                    package_id: "arknights.cn.full".to_string(),
                    execution_mode: "recognize_only".to_string(),
                    resolution: None,
                    out: out.clone(),
                    dry_run: false,
                    env: PackageEnvOptions::default(),
                },
            )
            .unwrap();
        assert_published_file(&out);
        let entries = read_zip_entries(&out);
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("operation task"),
        )
        .expect("task json");
        assert_eq!(task["goal"], "operator-cn");
        let index: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operations.index.json")
                .expect("operation index"),
        )
        .expect("index json");
        assert!(
            index["operations"]
                .as_array()
                .expect("index operations")
                .iter()
                .any(|entry| entry["task_id"] == "operator_task" && entry["goal"] == "operator-cn")
        );
    }

    #[test]
    fn catalog_packages_converter_synthesized_guards_without_mutating_source() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        update_fixture_operation(&repo, |task| {
            task["operations"][1]
                .as_object_mut()
                .expect("operation object")
                .remove("guard");
        });
        let out = temp.path().join("canonical-guard.zip");
        let catalog = PackageBuildCatalog::open(build_catalog_request(
            repo.clone(),
            temp.path().join("remote-source"),
        ))
        .expect("catalog");

        catalog
            .build_full_archive(
                &AuthoringEnvironmentSnapshot::default(),
                PackageFullArchiveRequest {
                    entry_task_id: "operator_task".to_string(),
                    package_id: "arknights.cn.full".to_string(),
                    execution_mode: "recognize_only".to_string(),
                    resolution: None,
                    out: out.clone(),
                    dry_run: false,
                    env: PackageEnvOptions::default(),
                },
            )
            .expect("full archive");

        let source: Value = serde_json::from_slice(
            &fs::read(repo.join("operations/operator_task/task.json")).expect("source task"),
        )
        .expect("source JSON");
        assert!(source["operations"][1].get("guard").is_none());
        let entries = read_zip_entries(&out);
        let packaged: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("packaged task"),
        )
        .expect("packaged JSON");
        assert_eq!(packaged["operations"][1]["guard"]["target_id"], "page/mall");
        assert_eq!(
            packaged["operations"][1]["guard"]["verify_template"],
            "assets/MALL.png"
        );
    }

    #[test]
    fn generated_environment_snapshot_covers_every_output_document() {
        let mut outputs = ConvertOutputs {
            pack: json!({"value": "{env:theme}"}),
            pages: json!({"value": "{env:theme}"}),
            navigation: json!({"value": "{env:theme}"}),
            index: json!({"value": "{env:theme}"}),
            primitives: json!({"value": "{env:theme}"}),
        };
        assert_eq!(
            generated_output_environment_keys(&outputs)
                .expect("generated environment keys")
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["theme".to_string()]
        );

        apply_environment_to_outputs(&environment_snapshot("theme", "Siege"), &mut outputs)
            .expect("apply environment");
        for output in [
            outputs.pack,
            outputs.pages,
            outputs.navigation,
            outputs.index,
            outputs.primitives,
        ] {
            assert_eq!(output["value"], "Siege");
        }
    }

    #[test]
    fn build_catalog_writes_one_task_archive_per_task() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let split_dir = temp.path().join("split");
        fs::create_dir_all(&split_dir).unwrap();
        let catalog = PackageBuildCatalog::open(build_catalog_request(
            repo,
            temp.path().join("remote-source"),
        ))
        .unwrap();
        let metadata = catalog.metadata();
        for task_id in catalog.task_ids() {
            let package_id = format!("{}.{}.{}", metadata.game, metadata.server, task_id);
            catalog
                .build_task_archive(
                    &AuthoringEnvironmentSnapshot::default(),
                    PackageTaskArchiveRequest {
                        task_id,
                        package_id: package_id.clone(),
                        execution_mode: "navigable_route".to_string(),
                        resolution: None,
                        out: split_dir.join(format!("{package_id}.zip")),
                        dry_run: false,
                        env: PackageEnvOptions::default(),
                    },
                )
                .unwrap();
        }
        assert_published_file(&split_dir.join("arknights.cn.operator_task.zip"));
        assert_published_file(&split_dir.join("arknights.cn.return_home.zip"));
    }

    #[test]
    fn build_task_rejects_dangerous_asset_entry() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        fs::write(
            repo.join("operations/operator_task/assets/bad.ps1"),
            "Write-Host bad",
        )
        .unwrap();

        let error = build_task(build_task_request(repo, temp.path().join("bad.zip")))
            .expect_err("dangerous package entry must fail");

        assert_eq!(error.code, "package_invalid");
    }

    #[test]
    fn stability_termination_accepts_full_and_edge_aligned_regions() {
        for region in [
            json!({"x": 0, "y": 0, "width": 1280, "height": 720}),
            json!({"x": 1279, "y": 719, "width": 1, "height": 1}),
        ] {
            let mut declaration = stability_termination_fixture();
            declaration["region"] = region;
            validate_stability_fixture(
                "in_page_guard",
                Some(declaration.clone()),
                Some(declaration),
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect("valid bounded stability termination");
        }
    }

    #[test]
    fn stability_termination_absence_preserves_existing_modes() {
        validate_stability_fixture("in_page_guard", None, None, None, |_| {}, |_| {})
            .expect("feature-absent in_page_guard remains valid");

        validate_stability_fixture("navigable_route", None, None, None, |_| {}, |_| {})
            .expect("legacy navigable route remains valid");

        let declaration = stability_termination_fixture();
        let wrong_mode = validate_stability_fixture(
            "navigable_route",
            Some(declaration.clone()),
            Some(declaration),
            Some(json!(4)),
            |_| {},
            |_| {},
        )
        .expect_err("other modes must not silently ignore stability termination");
        assert_eq!(wrong_mode.code, "package_invalid");
    }

    #[test]
    fn stability_termination_requires_every_typed_field() {
        for path in [
            "region",
            "region.x",
            "region.y",
            "region.width",
            "region.height",
            "comparison",
            "comparison.mode",
            "comparison.parameters",
            "consecutive_unchanged_threshold",
            "max_steps",
        ] {
            let mut task = stability_termination_fixture();
            let mut control = task.clone();
            remove_json_path(&mut task, path);
            remove_json_path(&mut control, path);
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(task),
                Some(control),
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect_err("missing stability field must fail");
            assert_eq!(error.code, "package_invalid", "path={path}");
        }
    }

    #[test]
    fn stability_termination_rejects_malformed_unknown_and_unsupported_values() {
        let cases = [
            ("null declaration", "", json!(null)),
            ("array declaration", "", json!([])),
            ("null region", "region", json!(null)),
            ("null comparison", "comparison", json!(null)),
            ("negative x", "region.x", json!(-1)),
            ("string width", "region.width", json!("1")),
            (
                "string threshold",
                "consecutive_unchanged_threshold",
                json!("2"),
            ),
            ("string max", "max_steps", json!("4")),
            (
                "unsupported mode",
                "comparison.mode",
                json!("perceptual_v1"),
            ),
            ("null parameters", "comparison.parameters", json!(null)),
            ("array parameters", "comparison.parameters", json!([])),
            (
                "nonempty parameters",
                "comparison.parameters",
                json!({"tolerance": 0}),
            ),
        ];
        for (label, path, replacement) in cases {
            let mut task = stability_termination_fixture();
            let mut control = task.clone();
            if path.is_empty() {
                task = replacement.clone();
                control = replacement;
            } else {
                set_json_path(&mut task, path, replacement.clone());
                set_json_path(&mut control, path, replacement);
            }
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(task),
                Some(control),
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect_err("malformed stability declaration must fail");
            assert_eq!(error.code, "package_invalid", "case={label}");
        }

        for (label, task, control) in [
            ("task null only", Some(json!(null)), None),
            ("control null only", None, Some(json!(null))),
        ] {
            let error = validate_stability_fixture(
                "in_page_guard",
                task,
                control,
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect_err("an explicit null declaration must not be treated as absence");
            assert_eq!(error.code, "package_invalid", "case={label}");
        }

        for path in ["", "region", "comparison", "comparison.parameters"] {
            let mut task = stability_termination_fixture();
            let mut control = task.clone();
            insert_unknown_json_field(&mut task, path);
            insert_unknown_json_field(&mut control, path);
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(task),
                Some(control),
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect_err("unknown stability field must fail");
            assert_eq!(error.code, "package_invalid", "path={path}");
        }
    }

    #[test]
    fn stability_termination_rejects_invalid_regions_and_checked_layout_overflow() {
        for (label, region) in [
            (
                "zero width",
                json!({"x": 0, "y": 0, "width": 0, "height": 1}),
            ),
            (
                "zero height",
                json!({"x": 0, "y": 0, "width": 1, "height": 0}),
            ),
            (
                "x outside",
                json!({"x": 1280, "y": 0, "width": 1, "height": 1}),
            ),
            (
                "y outside",
                json!({"x": 0, "y": 720, "width": 1, "height": 1}),
            ),
            (
                "right outside",
                json!({"x": 1279, "y": 0, "width": 2, "height": 1}),
            ),
            (
                "bottom outside",
                json!({"x": 0, "y": 719, "width": 1, "height": 2}),
            ),
            (
                "checked x overflow",
                json!({"x": u32::MAX, "y": 0, "width": 2, "height": 1}),
            ),
            (
                "checked y overflow",
                json!({"x": 0, "y": u32::MAX, "width": 1, "height": 2}),
            ),
        ] {
            let mut task = stability_termination_fixture();
            task["region"] = region;
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(task.clone()),
                Some(task),
                Some(json!(4)),
                |_| {},
                |_| {},
            )
            .expect_err("invalid region must fail before execution");
            assert_eq!(error.code, "package_invalid", "case={label}");
        }

        let mut task = stability_termination_fixture();
        task["region"] = json!({
            "x": 0,
            "y": 0,
            "width": u32::MAX,
            "height": u32::MAX
        });
        let error = validate_stability_fixture(
            "in_page_guard",
            Some(task.clone()),
            Some(task),
            Some(json!(4)),
            |control| {
                control["resolution"] = json!({"width": u32::MAX, "height": u32::MAX});
            },
            |operation| {
                operation["coordinate_space"] = json!({"width": u32::MAX, "height": u32::MAX});
            },
        )
        .expect_err("checked pixel-layout overflow must fail before execution");
        assert_eq!(error.code, "package_invalid");
    }

    #[test]
    fn stability_termination_rejects_threshold_and_hard_cap_bounds() {
        for (label, threshold, max_steps) in [
            ("zero threshold", 0, 4),
            ("zero max", 1, 0),
            ("one max", 1, 1),
            ("max above limit", 1, 1001),
            ("threshold equals max", 4, 4),
            ("threshold above max", 5, 4),
        ] {
            let mut declaration = stability_termination_fixture();
            declaration["consecutive_unchanged_threshold"] = json!(threshold);
            declaration["max_steps"] = json!(max_steps);
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(declaration.clone()),
                Some(declaration),
                Some(json!(max_steps)),
                |_| {},
                |_| {},
            )
            .expect_err("invalid threshold/hard-cap relation must fail");
            assert_eq!(error.code, "package_invalid", "case={label}");
        }
    }

    #[test]
    fn stability_termination_rejects_target_and_scheduling_conflicts() {
        for field in ["target_page", "scheduling_outcome"] {
            let declaration = stability_termination_fixture();
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(declaration.clone()),
                Some(declaration),
                Some(json!(4)),
                |_| {},
                |operation| {
                    operation[field] = if field == "target_page" {
                        json!("terminal")
                    } else {
                        json!({"designated_operation": null, "mappings": []})
                    };
                },
            )
            .expect_err("target ownership or an unowned scheduling outcome must fail closed");
            assert_eq!(error.code, "package_invalid", "field={field}");
        }
    }

    #[test]
    fn stability_termination_accepts_only_the_post_admission_ocr_owned_outcome() {
        let declaration = stability_termination_fixture();
        let post_admission_ocr = |page_id: &str, outcome_key: &str| {
            json!({
                "page_id": page_id,
                "target_id": "ocr/synthetic-stability",
                "truth_set": {"path": "truth.json", "sha256": "a".repeat(64)},
                "normalization": "trim_lowercase_v1",
                "comparison": "exact_set_v1",
                "limits": {
                    "max_frames": 2,
                    "max_items": 16,
                    "max_string_bytes": 64,
                    "max_total_bytes": 4096,
                    "max_truth_entries": 16
                },
                "outcome_key": outcome_key
            })
        };
        let scheduling_outcome = |page_id: &str, outcome_keys: &[&str]| {
            json!({
                "mappings": outcome_keys
                    .iter()
                    .map(|outcome_key| json!({
                        "outcome_key": outcome_key,
                        "effect": "no_designated_effect",
                        "terminal_pages": [page_id]
                    }))
                    .collect::<Vec<_>>()
            })
        };

        validate_stability_fixture(
            "in_page_guard",
            Some(declaration.clone()),
            Some(declaration.clone()),
            Some(json!(4)),
            |_| {},
            |operation| {
                let admitted_page = operation["operations"][0]["from"]
                    .as_str()
                    .expect("fixture admitted page")
                    .to_string();
                operation["schema_version"] = json!("0.7");
                operation["scheduling_outcome"] =
                    scheduling_outcome(&admitted_page, &["comparison_recorded"]);
                operation["post_admission_ocr"] =
                    post_admission_ocr(&admitted_page, "comparison_recorded");
                operation["operations"][0]["to"] = json!(admitted_page);
            },
        )
        .expect("the OCR declaration owns its one mapped scheduling result");

        for (case, mappings, selected) in [
            ("missing", Vec::new(), "comparison_recorded"),
            (
                "duplicate",
                vec!["comparison_recorded", "comparison_recorded"],
                "comparison_recorded",
            ),
            ("mismatch", vec!["other_result"], "comparison_recorded"),
        ] {
            let error = validate_stability_fixture(
                "in_page_guard",
                Some(declaration.clone()),
                Some(declaration.clone()),
                Some(json!(4)),
                |_| {},
                |operation| {
                    let admitted_page = operation["operations"][0]["from"]
                        .as_str()
                        .expect("fixture admitted page")
                        .to_string();
                    operation["schema_version"] = json!("0.7");
                    operation["scheduling_outcome"] = scheduling_outcome(&admitted_page, &mappings);
                    operation["post_admission_ocr"] = post_admission_ocr(&admitted_page, selected);
                    operation["operations"][0]["to"] = json!(admitted_page);
                },
            )
            .expect_err("missing, duplicate, or mismatched ownership must fail closed");
            assert_eq!(error.code, "package_invalid", "case={case}");
        }
    }

    #[test]
    fn stability_termination_rejects_control_task_and_root_cap_tampering() {
        let declaration = stability_termination_fixture();
        for (label, task, control, root_max) in [
            ("task only", Some(declaration.clone()), None, Some(json!(4))),
            (
                "control only",
                None,
                Some(declaration.clone()),
                Some(json!(4)),
            ),
            (
                "declaration mismatch",
                Some(declaration.clone()),
                Some({
                    let mut changed = declaration.clone();
                    changed["consecutive_unchanged_threshold"] = json!(1);
                    changed
                }),
                Some(json!(4)),
            ),
            (
                "missing root max",
                Some(declaration.clone()),
                Some(declaration.clone()),
                None,
            ),
            (
                "root max mismatch",
                Some(declaration.clone()),
                Some(declaration.clone()),
                Some(json!(5)),
            ),
        ] {
            let error = validate_stability_fixture(
                "in_page_guard",
                task,
                control,
                root_max,
                |_| {},
                |_| {},
            )
            .expect_err("task/control tampering must fail");
            assert_eq!(error.code, "package_invalid", "case={label}");
        }
    }

    #[test]
    fn in_page_guard_build_preserves_declaration_and_projects_control() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let declaration = stability_termination_fixture();
        update_fixture_operation(&repo, |operation| {
            operation
                .as_object_mut()
                .expect("operation object")
                .remove("target_page");
            operation["stability_termination"] = declaration.clone();
        });
        let out = temp.path().join("stability.zip");
        let mut request = build_task_request(repo, out.clone());
        request.execution_mode = Some("in_page_guard".to_string());

        build_task(request).expect("build in-page stability package");

        let entries = read_zip_entries(&out);
        let control: Value =
            serde_json::from_slice(entries.get("control.json").expect("packaged control"))
                .expect("control JSON");
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("packaged task"),
        )
        .expect("task JSON");
        assert_eq!(task["stability_termination"], declaration);
        assert_eq!(control["stability_termination"], declaration);
        assert_eq!(control["max_steps"], json!(4));
    }

    #[test]
    fn in_page_guard_build_accepts_post_admission_ocr_owned_stability_outcome() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let out = temp.path().join("ocr-stability.zip");
        let mut request = build_task_request(repo.clone(), out.clone());
        let task_id = request.task_id.clone();
        let truth = serde_json::to_vec(&json!({
            "schema_version": "actingcommand.ocr-truth-set.v1",
            "items": ["synthetic truth"]
        }))
        .expect("truth bytes");
        fs::write(
            repo.join("operations").join(&task_id).join("truth.json"),
            &truth,
        )
        .expect("truth fixture");
        let truth_sha256 = hex_sha256(&truth);
        let declaration = stability_termination_fixture();
        update_fixture_operation(&repo, |task| {
            let admitted_page = task["entry_page"]
                .as_str()
                .expect("fixture entry page")
                .to_string();
            task.as_object_mut()
                .expect("task object")
                .remove("target_page");
            task["schema_version"] = json!("0.7");
            task["stability_termination"] = declaration.clone();
            task["operations"][0]["to"] = json!(admitted_page);
            task["operations"][0]["post_delay_ms"] = json!(250);
            task["ocr_targets"] = json!([{
                "id": "ocr/synthetic-stability",
                "region": {
                    "mode": "rect",
                    "rect": {"x": 20, "y": 30, "width": 200, "height": 40}
                },
                "languages": ["en"],
                "timeout_ms": 5_000,
                "match_mode": "exact",
                "expected": ["synthetic truth"],
                "case_sensitive": false,
                "minimum_confidence": 0.8,
                "model_ref": "PP-OCRv6_medium",
                "model_sha256": "a".repeat(64)
            }]);
            task["scheduling_outcome"] = json!({
                "mappings": [{
                    "outcome_key": "comparison_recorded",
                    "effect": "no_designated_effect",
                    "terminal_pages": [task["entry_page"].clone()]
                }]
            });
            task["post_admission_ocr"] = json!({
                "page_id": task["entry_page"].clone(),
                "target_id": "ocr/synthetic-stability",
                "truth_set": {"path": "truth.json", "sha256": truth_sha256},
                "normalization": "trim_lowercase_v1",
                "comparison": "exact_set_v1",
                "limits": {
                    "max_frames": 2,
                    "max_items": 16,
                    "max_string_bytes": 64,
                    "max_total_bytes": 4096,
                    "max_truth_entries": 16
                },
                "outcome_key": "comparison_recorded"
            });
        });
        request.execution_mode = Some("in_page_guard".to_string());

        let response = build_task(request).expect("official source and generated validation");
        assert_eq!(response.validation.status, "valid");
        let inspected = crate::package_validate::validate_package(crate::PackageValidateRequest {
            zip_path: out.clone(),
            include_entries: true,
            expected_input_sha256: None,
        })
        .expect("official package validate/inspect path");
        assert_eq!(inspected.status, "valid");
        let entries = read_zip_entries(&out);
        let control: Value =
            serde_json::from_slice(entries.get("control.json").expect("packaged control"))
                .expect("control JSON");
        let task_path = format!("resources/operations/{task_id}/task.json");
        let task: Value = serde_json::from_slice(entries.get(&task_path).expect("packaged task"))
            .expect("task JSON");
        assert_eq!(control["stability_termination"], declaration);
        assert_eq!(task["stability_termination"], declaration);
        assert_eq!(task["operations"][0]["post_delay_ms"], json!(250));
        assert_eq!(
            task["post_admission_ocr"]["outcome_key"],
            "comparison_recorded"
        );
        assert_eq!(
            task["scheduling_outcome"]["mappings"][0]["outcome_key"],
            "comparison_recorded"
        );

        update_fixture_operation(&repo, |task| {
            task["schema_version"] = json!("0.6");
            task.as_object_mut()
                .expect("task object")
                .remove("post_admission_ocr");
        });
        let unowned_out = temp.path().join("unowned-stability-outcome.zip");
        let mut unowned_request = build_task_request(repo, unowned_out.clone());
        unowned_request.execution_mode = Some("in_page_guard".to_string());
        let error = build_task(unowned_request)
            .expect_err("stability plus scheduling without OCR ownership must fail closed");
        assert_eq!(error.code, "package_invalid");
        assert!(error.message.contains("conflicts with scheduling_outcome"));
        assert!(!unowned_out.exists());
    }

    #[test]
    fn in_page_guard_build_without_declaration_preserves_legacy_control() {
        let temp = TempDir::new().expect("temp");
        let repo = temp.path().join("repo");
        write_fixture_repo(&repo);
        let out = temp.path().join("missing-stability.zip");
        let mut request = build_task_request(repo, out.clone());
        request.execution_mode = Some("in_page_guard".to_string());

        build_task(request).expect("feature-absent in_page_guard package");

        let entries = read_zip_entries(&out);
        let control: Value =
            serde_json::from_slice(entries.get("control.json").expect("packaged control"))
                .expect("control JSON");
        let task: Value = serde_json::from_slice(
            entries
                .get("resources/operations/operator_task/task.json")
                .expect("packaged task"),
        )
        .expect("task JSON");
        assert!(control.get("stability_termination").is_none());
        assert!(control.get("max_steps").is_none());
        assert!(task.get("stability_termination").is_none());
        assert_eq!(task["target_page"], json!("mall"));
    }

    fn read_zip_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut package = open_published_package(path).unwrap();
        let mut zip = ZipArchive::new(package.file_mut()).unwrap();
        let mut entries = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).unwrap();
            if entry.name().ends_with('/') {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            entries.insert(entry.name().to_string(), bytes);
        }
        drop(zip);
        package.close().unwrap();
        entries
    }

    fn stability_termination_fixture() -> Value {
        json!({
            "region": {"x": 0, "y": 0, "width": 1280, "height": 720},
            "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
            "consecutive_unchanged_threshold": 2,
            "max_steps": 4
        })
    }

    fn stability_operation_fixture(stability_termination: Option<Value>) -> Value {
        let mut operation = json!({
            "schema_version": "0.6",
            "task_id": "operator_task",
            "game": "arknights",
            "server_scope": ["cn"],
            "coordinate_space": {"width": 1280, "height": 720},
            "operations": [{
                "id": "scroll_once",
                "purpose": "advance one bounded page",
                "from": "operator",
                "click": {"kind": "point", "x": 1, "y": 1},
                "unguarded_trusted_coordinate": true
            }]
        });
        if let Some(stability_termination) = stability_termination {
            operation["stability_termination"] = stability_termination;
        }
        operation
    }

    fn stability_control_fixture(
        execution_mode: &str,
        stability_termination: Option<Value>,
        max_steps: Option<Value>,
    ) -> Value {
        let mut control = json!({
            "schema_version": CONTROL_SCHEMA,
            "package_id": "arknights.cn.operator_task",
            "execution_mode": execution_mode,
            "game": "arknights",
            "server": "cn",
            "resolution": {"width": 1280, "height": 720},
            "entry_task_id": "operator_task"
        });
        if let Some(stability_termination) = stability_termination {
            control["stability_termination"] = stability_termination;
        }
        if let Some(max_steps) = max_steps {
            control["max_steps"] = max_steps;
        }
        control
    }

    fn validate_stability_fixture(
        execution_mode: &str,
        task_stability: Option<Value>,
        control_stability: Option<Value>,
        control_max_steps: Option<Value>,
        update_control: impl FnOnce(&mut Value),
        update_operation: impl FnOnce(&mut Value),
    ) -> CliOutcome<()> {
        let mut control =
            stability_control_fixture(execution_mode, control_stability, control_max_steps);
        update_control(&mut control);
        let control: LabControl = serde_json::from_value(control).map_err(|error| {
            CliError::package_invalid(format!("failed to parse stability control: {error}"))
        })?;
        control.validate()?;

        let mut operation = stability_operation_fixture(task_stability);
        update_operation(&mut operation);
        let operation: OperationBundle = serde_json::from_value(operation).map_err(|error| {
            CliError::package_invalid(format!("failed to parse stability operation: {error}"))
        })?;
        operation.validate(&control, |_| Ok(true))
    }

    fn json_path_parent_mut<'a>(value: &'a mut Value, path: &str) -> (&'a mut Value, String) {
        let (parent, leaf) = path.rsplit_once('.').unwrap_or(("", path));
        let mut cursor = value;
        if !parent.is_empty() {
            for segment in parent.split('.') {
                cursor = cursor.get_mut(segment).expect("fixture JSON path segment");
            }
        }
        (cursor, leaf.to_string())
    }

    fn remove_json_path(value: &mut Value, path: &str) {
        let (parent, leaf) = json_path_parent_mut(value, path);
        parent
            .as_object_mut()
            .expect("fixture JSON object")
            .remove(&leaf);
    }

    fn set_json_path(value: &mut Value, path: &str, replacement: Value) {
        let (parent, leaf) = json_path_parent_mut(value, path);
        parent
            .as_object_mut()
            .expect("fixture JSON object")
            .insert(leaf, replacement);
    }

    fn insert_unknown_json_field(value: &mut Value, path: &str) {
        let target = if path.is_empty() {
            value
        } else {
            let mut cursor = value;
            for segment in path.split('.') {
                cursor = cursor.get_mut(segment).expect("fixture JSON path segment");
            }
            cursor
        };
        target
            .as_object_mut()
            .expect("fixture JSON object")
            .insert("unsupported".to_string(), json!(true));
    }

    fn assert_published_file(path: &Path) {
        let package = open_published_package(path).unwrap();
        assert!(package.path().is_file());
        package.close().unwrap();
    }

    fn write_fixture_repo(root: &Path) {
        fs::create_dir_all(root.join("operations/operator_task/assets")).unwrap();
        fs::create_dir_all(root.join("operations/return_home/assets")).unwrap();
        fs::create_dir_all(root.join("navigation")).unwrap();
        fs::write(
            root.join("operations/resources.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "1.0",
                "resources": [
                    {"id": "sanity", "name": {"cn": "sanity"}},
                    {"id": "credit", "name": {"cn": "credit"}}
                ],
                "resource_count": 2
            }))
            .unwrap(),
        )
        .unwrap();
        for path in [
            "operations/operator_task/assets/PAGE_OPERATOR_0.png",
            "operations/operator_task/assets/PAGE_OPERATOR_1.png",
            "operations/operator_task/assets/MIDDLE.png",
            "operations/operator_task/assets/MALL.png",
            "operations/return_home/assets/HOME.png",
        ] {
            fs::write(root.join(path), one_pixel_png()).unwrap();
        }
        fs::write(
            root.join("operations/operator_task/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "operator_task",
                "game": "arknights",
                "server_scope": ["cn"],
                "locale": "zh-CN",
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "operator_0", "template": "assets/PAGE_OPERATOR_0.png", "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}}, "threshold": 0.8, "color_check": null},
                    {"id": "operator_1", "template": "assets/PAGE_OPERATOR_1.png", "region": {"mode": "rect", "rect": {"x": 5, "y": 6, "width": 7, "height": 8}}, "threshold": 0.8, "color_check": null},
                    {"id": "middle", "template": "assets/MIDDLE.png", "region": {"mode": "rect", "rect": {"x": 9, "y": 10, "width": 11, "height": 12}}, "threshold": 0.8, "color_check": null},
                    {"id": "mall", "template": "assets/MALL.png", "region": {"mode": "rect", "rect": {"x": 13, "y": 14, "width": 15, "height": 16}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "operator",
                "target_page": "mall",
                "operations": [
                    {"id": "operator_to_middle", "purpose": "go middle", "from": "operator", "to": "middle", "click": {"kind": "rect", "x": 100, "y": 100, "width": 20, "height": 20}, "verify_template": null, "guard": {"page_id": "operator", "target_id": "page/operator_0", "expected_rect": {"x": 100, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/PAGE_OPERATOR_0.png"}, "consumes": [], "produces": ["credit"]},
                    {"id": "middle_to_mall", "purpose": "go mall", "from": "middle", "to": "mall", "click": {"kind": "rect", "x": 200, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/MALL.png", "guard": {"page_id": "middle", "target_id": "page/middle", "expected_rect": {"x": 200, "y": 100, "width": 20, "height": 20}, "verify_template": "assets/MIDDLE.png"}, "consumes": ["sanity"], "produces": []}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("operations/return_home/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "return_home",
                "game": "arknights",
                "server_scope": ["cn"],
                "locale": "zh-CN",
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "home", "template": "assets/HOME.png", "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "home",
                "target_page": "home",
                "operations": [
                    {"id": "home_noop", "purpose": "noop", "from": "home", "to": null, "click": {"kind": "point", "x": 1, "y": 1}, "verify_template": null, "guard": {"page_id": "home", "target_id": "page/home", "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1}, "verify_template": "assets/HOME.png"}, "consumes": [], "produces": []}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("navigation/arknights.cn.navigation.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "control_points": [{"name": "home", "point": [1, 1]}]
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn update_fixture_operation(root: &Path, update: impl FnOnce(&mut Value)) {
        let path = root.join("operations/operator_task/task.json");
        let mut operation: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        update(&mut operation);
        fs::write(path, serde_json::to_string_pretty(&operation).unwrap()).unwrap();
    }

    fn set_fixture_page_declaration(task: &mut Value, field: &str, declaration: Value) {
        match field {
            "target_page" => task["target_page"] = declaration,
            "to" => task["operations"][0]["to"] = declaration,
            "expect_after.page_id" => {
                task["operations"][0]["expect_after"] = json!({"page_id": declaration});
            }
            other => panic!("unsupported fixture field {other}"),
        }
    }

    fn validate_packaged_operation_fixture(task: Value) -> CliOutcome<()> {
        let control: LabControl = serde_json::from_value(control_json(
            "arknights.cn.operator_task",
            "navigable_route",
            "arknights",
            "cn",
            (1280, 720),
            "operator_task",
            None,
        )?)
        .expect("fixture control");
        let bundle: OperationBundle = serde_json::from_value(task).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to parse packaged operation fixture: {error}"
            ))
        })?;
        bundle.validate(&control, |_| Ok(true))
    }

    fn environment_snapshot(key: &str, value: &str) -> AuthoringEnvironmentSnapshot {
        AuthoringEnvironmentSnapshot::from_resolved([actingcommand_contract::EnvResolved {
            key: key.to_string(),
            value: value.to_string(),
            confidence: 1.0,
            source: "sealed".to_string(),
            detector_id: "test".to_string(),
            source_result: "test@1".to_string(),
        }])
        .expect("environment snapshot")
    }

    fn add_recruit_fixture(root: &Path) {
        fs::create_dir_all(root.join("operations/recruit_task/assets")).unwrap();
        fs::write(
            root.join("operations/recruit_task/assets/RECRUIT.png"),
            one_pixel_png(),
        )
        .unwrap();
        fs::write(
            root.join("operations/recruit_task/task.json"),
            serde_json::to_string_pretty(&json!({
                "schema_version": "0.3",
                "task_id": "recruit_task",
                "game": "arknights",
                "server_scope": ["cn"],
                "locale": "zh-CN",
                "goal": "fixture",
                "coordinate_space": {"width": 1280, "height": 720},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
                "anchors": [
                    {"id": "recruit", "template": "assets/RECRUIT.png", "region": {"mode": "rect", "rect": {"x": 40, "y": 40, "width": 50, "height": 50}}, "threshold": 0.8, "color_check": null}
                ],
                "entry_page": "recruit",
                "target_page": "recruit",
                "operations": []
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn add_return_home_recruit_rule(root: &Path) {
        let path = root.join("operations/return_home/task.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().insert(
            "page_rules".to_string(),
            json!({
                "home": {"required": ["page/home"]},
                "recruit": {"required": ["page/recruit"], "forbidden": ["page/home"]}
            }),
        );
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn one_pixel_png() -> &'static [u8] {
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4,
            0, 9, 251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }
}
