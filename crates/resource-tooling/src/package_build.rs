// SPDX-License-Identifier: AGPL-3.0-only

use crate::environment::{AuthoringEnvironmentSnapshot, required_environment_keys};
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
use actingcommand_contract::{LabError as CliError, LabResult as CliOutcome};
use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, Sha256Hash};
use actingcommand_recognition_pack::PackRect;
use serde::Deserialize;
#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use zip::ZipWriter;
use zip::write::FileOptions;

const DANGEROUS_EXTENSIONS: &[&str] = &[
    "py", "exe", "bat", "cmd", "ps1", "sh", "js", "vbs", "msi", "dll", "scr", "com", "jar",
];
const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const DEFAULT_TEMPLATE_THRESHOLD: f32 = 0.9;
const DEFAULT_RECOVERY_TASK_ID: &str = "return_home";

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
        out,
        dry_run,
        max_buffered_payload_bytes,
    })
}

impl PreparedPackageBuildTask {
    pub fn resource_root(&self) -> &Path {
        &self.resource_root
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
            ),
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

        let write = write_and_validate_package(&self.out, entries, self.dry_run)?;
        let from_remote = self.source.remote_url();
        self.source.cleanup()?;
        Ok(PackageBuildTaskResponse {
            status: if self.dry_run { "validated" } else { "written" }.to_string(),
            mode: "build-task".to_string(),
            repo: self.repo.display().to_string(),
            resource_root: self.resource_root.display().to_string(),
            resource_layout: self.resource_layout,
            from_remote,
            task_id: self.task_id,
            included_tasks: self.task_ids,
            game: self.converter.game,
            server: self.converter.server,
            package_id: self.package_id,
            execution_mode: self.execution_mode,
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

    pub fn default_entry_task(&self) -> String {
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
            ),
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
        Ok(write_and_validate_package(&out, entries, dry_run)?.validation)
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
            ),
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

fn default_entry_task(converter: &OperationConverter) -> String {
    if converter
        .bundles
        .iter()
        .any(|bundle| bundle.task_id == "return_home")
    {
        "return_home".to_string()
    } else {
        converter
            .bundles
            .first()
            .map(|bundle| bundle.task_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}.{}",
                    converter.game,
                    default_server_for_game(&converter.game)
                )
            })
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
) -> Value {
    ordered_object([
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
    ])
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackagePayloadStats {
    logical_payload_bytes: u64,
    buffered_payload_bytes: usize,
    peak_buffered_payload_bytes: usize,
    peak_streamed_payload_bytes: usize,
    peak_resident_payload_bytes: usize,
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
}

fn write_and_validate_package(
    out: &Path,
    mut entries: PackageEntries,
    dry_run: bool,
) -> CliOutcome<PackageWrite> {
    let temp = temp_zip_path(out)?;
    write_zip(&temp, &mut entries)?;
    let validation = match validate_generated_package(&temp) {
        Ok(value) => value,
        Err(err) => {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
    };
    if dry_run {
        fs::remove_file(&temp).map_err(|err| {
            CliError::package_invalid(format!("failed to remove {}: {err}", temp.display()))
        })?;
        return Ok(PackageWrite { validation });
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    if out.exists() {
        if out.is_dir() {
            let _ = fs::remove_file(&temp);
            return Err(CliError::package_invalid(format!(
                "output path is a directory: {}",
                out.display()
            )));
        }
        fs::remove_file(out).map_err(|err| {
            let _ = fs::remove_file(&temp);
            CliError::package_invalid(format!("failed to replace {}: {err}", out.display()))
        })?;
    }
    fs::rename(&temp, out).map_err(|err| {
        let _ = fs::remove_file(&temp);
        CliError::package_invalid(format!(
            "failed to move {} to {}: {err}",
            temp.display(),
            out.display()
        ))
    })?;
    Ok(PackageWrite { validation })
}

fn write_zip(path: &Path, entries: &mut PackageEntries) -> CliOutcome<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let file = File::create(path).map_err(|err| {
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
    zip.finish().map_err(zip_write_error)?;
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

fn validate_generated_package(path: &Path) -> CliOutcome<LabPackageValidationResponse> {
    let bytes = fs::read(path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to read Lab package {}: {error}",
            path.display()
        ))
    })?;
    let expected = Sha256Hash::digest(&bytes);
    let instance = InstanceId::new("lab-validate").map_err(containment_error)?;
    let mut containment = Containment::new();
    let bundle = containment
        .load(&instance, &bytes, &expected)
        .map_err(containment_error)?;
    let control = lab_control_from_bundle(bundle)?;
    control.validate()?;
    validate_manifest_entry_task_id(
        Path::new(bundle.manifest_path()),
        bundle.manifest(),
        &control,
    )?;
    let operation_bundle: OperationBundle = serde_json::from_value(bundle.operation().clone())
        .map_err(|error| {
            CliError::package_invalid(format!(
                "failed to parse {}: {error}",
                bundle.operation_path()
            ))
        })?;
    operation_bundle.validate(&control, |relative| {
        bundle
            .resource_entry(&format!(
                "operations/{}/{}",
                control.entry_task_id, relative
            ))
            .map(|_| true)
            .or_else(|error| match error {
                ContainmentError::MissingEntry { .. } => Ok(false),
                other => Err(containment_error(other)),
            })
    })?;
    let evaluator = bundle.evaluator().ok_or_else(|| {
        CliError::package_invalid("missing recognition evaluator for Lab package")
    })?;
    bundle
        .detector()
        .ok_or_else(|| CliError::package_invalid("missing page detector for Lab package"))?;
    let unsupported_targets = evaluator
        .unsupported_targets()
        .iter()
        .map(|target| UnsupportedRecognitionTargetResponse {
            id: target.id.clone(),
            reason: target.reason.clone(),
        })
        .collect::<Vec<_>>();
    let pack = bundle
        .recognition_pack_path()
        .ok_or_else(|| CliError::package_invalid("missing recognition pack for Lab package"))?;
    let pages = bundle
        .pages_path()
        .ok_or_else(|| CliError::package_invalid("missing page set for Lab package"))?;
    Ok(LabPackageValidationResponse {
        zip: path.display().to_string(),
        status: "valid".to_string(),
        entry_count: bundle.entry_count(),
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
            resource_root: bundle.resource_root().to_string(),
            manifest: bundle.manifest_path().to_string(),
            operation: bundle.operation_path().to_string(),
            operation_count: operation_bundle.operations.len(),
            pack: pack.to_string(),
            recognition_unsupported_target_count: unsupported_targets.len(),
            recognition_unsupported_targets: unsupported_targets,
            pages: pages.to_string(),
            navigation: bundle.navigation_path().map(str::to_string),
        },
    })
}

fn lab_control_from_bundle(
    bundle: &actingcommand_pack_containment::LoadedBundle,
) -> CliOutcome<LabControl> {
    let control = bundle
        .control()
        .ok_or_else(|| CliError::package_invalid("Lab package must include control.json"))?;
    serde_json::from_value(control.clone()).map_err(|error| {
        CliError::package_invalid(format!("failed to parse control.json: {error}"))
    })
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
        if let Some(capture_backend) = &self.capture_backend {
            validate_capture_backend(capture_backend)?;
        }
        self.frame_store
            .validate()
            .map_err(CliError::package_invalid)
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
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
    target_page: Option<String>,
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
        if !matches!(self.schema_version.as_str(), "0.3" | "0.4" | "0.5" | "0.6") {
            return Err(CliError::package_invalid(format!(
                "unsupported operation schema_version '{}', expected one of 0.3, 0.4, 0.5, 0.6",
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
        self.defaults.validate()?;
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
    to: Option<String>,
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
        if let Some(expect_after) = &self.expect_after {
            expect_after.validate(&self.id)?;
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
struct OperationExpectation {
    page_id: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl OperationExpectation {
    fn validate(&self, operation_id: &str) -> CliOutcome<()> {
        if self.page_id.trim().is_empty() {
            return Err(CliError::package_invalid(format!(
                "operation '{operation_id}' expect_after.page_id must not be empty"
            )));
        }
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
    #[serde(default, rename = "from")]
    from_rect: Option<PackRect>,
    #[serde(default, rename = "to")]
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
            let _ = fs::remove_dir_all(&root);
            return Err(CliError::usage(format!(
                "git clone failed: {}",
                stderr.trim()
            )));
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
        if let Some(root) = self.temp_root.take() {
            let _ = fs::remove_dir_all(root);
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

fn default_server_for_game(game: &str) -> &'static str {
    match game {
        "arknights" => "cn",
        "azurlane" | "bluearchive" => "jp",
        _ => "jp",
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
    fn streamed_payload_peak_is_bounded_when_total_exceeds_four_budgets() {
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
        entries.add_manifest("operator_task").expect("manifest");
        let out = temp.path().join("bounded.zip");
        write_zip(&out, &mut entries).expect("bounded zip");

        let logical_source_bytes = source_size * source_count;
        assert!(logical_source_bytes >= budget * 4);
        assert!(
            entries.stats.logical_payload_bytes
                >= u64::try_from(logical_source_bytes).expect("logical bytes")
        );
        assert!(entries.stats.peak_buffered_payload_bytes <= budget);
        assert!(entries.stats.peak_streamed_payload_bytes <= PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES);
        assert!(
            entries.stats.peak_resident_payload_bytes
                <= budget + PACKAGE_COMPRESSOR_INPUT_BUFFER_BYTES
        );
        assert!(out.is_file());
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
        assert!(out.is_file());
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
        assert!(out.is_file());
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
        assert!(out.is_file());
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
        assert!(split_dir.join("arknights.cn.operator_task.zip").is_file());
        assert!(split_dir.join("arknights.cn.return_home.zip").is_file());
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

    fn read_zip_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let file = File::open(path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
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
        entries
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
