// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only adapters from repository paths to the production declaration parsers.

use crate::{CliError, CliOutcome, FlagArgs};
use actingcommand_contract::{
    LabErrorClass, ResourceDeclarationIssue, page_projection::ProjectionMetadata,
    resource_declaration::ProcedureBindingConfigFile,
};
use actingcommand_lab::{
    DriveNavigationGraph, parse_environment_catalog_value, validate_control_declaration,
};
use actingcommand_pack_containment::source::{
    Bundle, ConversionFiles, SourceFile, SourceRead, declaration_file_requests,
    validate_bundle_declarations, validate_control_declarations, validate_navigation_declarations,
    validate_resource_declarations,
};
use actingcommand_policy::{
    CatalogDocumentSource, SchedulingDocumentKind, validate_catalog_declaration,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const MAX_PATHS: usize = 4_096;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PATH_LIST_BYTES: u64 = 1024 * 1024;

pub(crate) fn options() -> [&'static str; 3] {
    [
        "--repo <repository root>",
        "--changed-path <repository-relative path> (repeatable)",
        "--changed-paths-file <repository-relative NUL-delimited Git name-status list>",
    ]
}

pub(crate) fn run_resource_validation(repo: &Path, flags: &FlagArgs) -> CliOutcome<Value> {
    for flag in flags.flags.keys() {
        if !matches!(
            flag.as_str(),
            "--repo" | "--changed-path" | "--changed-paths-file"
        ) {
            return Err(CliError::usage(format!(
                "resource validate does not accept {flag}"
            )));
        }
    }
    let mut reader = DeclarationReader::new(repo)?;
    let mut selected = flags
        .values("--changed-path")
        .into_iter()
        .map(|path| (path, false))
        .collect::<Vec<_>>();
    if let Some(list) = flags.optional("--changed-paths-file") {
        let bytes = reader.read(Path::new(&list), MAX_PATH_LIST_BYTES)?;
        if !bytes.is_empty() && bytes.last() != Some(&0) {
            return Err(invalid(
                Path::new(&list),
                "changed path list must end with NUL",
            ));
        }
        let fields = bytes
            .strip_suffix(&[0])
            .map(|bytes| bytes.split(|byte| *byte == 0).collect::<Vec<_>>())
            .unwrap_or_default();
        let (pairs, remainder) = fields.as_chunks::<2>();
        if !remainder.is_empty() || pairs.len() > MAX_PATHS {
            return Err(invalid(
                Path::new(&list),
                "Git name-status list must contain at most 4096 status/path pairs",
            ));
        }
        for pair in pairs {
            let deleted = match pair[0] {
                b"D" => true,
                b"A" | b"M" | b"T" => false,
                _ => {
                    return Err(invalid(
                        Path::new(&list),
                        "unsupported Git status; use --no-renames and resolved changes",
                    ));
                }
            };
            let path = std::str::from_utf8(pair[1])
                .map_err(|_| invalid(Path::new(&list), "changed path is not UTF-8"))?;
            selected.push((path.to_string(), deleted));
        }
    } else if selected.is_empty() {
        return Err(CliError::usage(
            "resource validate requires --changed-path or --changed-paths-file",
        ));
    }
    if selected.len() > MAX_PATHS {
        return Err(CliError::usage(
            "changed declaration selection exceeds 4096 paths",
        ));
    }
    let mut paths = BTreeMap::new();
    for (path, deleted) in selected {
        validate_relative_path(Path::new(&path))?;
        if let Some(previous) = paths.insert(PathBuf::from(&path), deleted)
            && previous != deleted
        {
            return Err(invalid(
                Path::new(&path),
                "changed path has conflicting Git statuses",
            ));
        }
    }
    let mut entries = Vec::new();
    for (path, deleted) in paths {
        if deleted && exclusion(&path).is_none() {
            if reader.existing_file(&path)? {
                return Err(invalid(
                    &path,
                    "Git deletion still exists in the selected resource tree",
                ));
            }
            entries.push(reader.removal(&path)?);
        } else {
            entries.push(reader.validate(&path)?);
        }
    }
    Ok(json!({
        "schema_version": "actinglab.resource-declarations.v1",
        "scope": "declarations_only",
        "repo": reader.root,
        "status": "valid",
        "entries": entries,
        "read_files": reader.read_files,
        "read_bytes": reader.read_bytes,
    }))
}

pub(crate) fn operation(repo: &Path, operation_dir: &Path) -> CliOutcome<Value> {
    let mut reader = DeclarationReader::new(repo)?;
    validate_relative_path(operation_dir)?;
    let path = operation_dir.join("task.json");
    let data = reader.json(&path)?;
    if crate::resource_runtime_support::contains_string_value(&data, "unresolved_coords") {
        return Err(CliError::safety_blocked(
            "unresolved_coords",
            "operation contains unresolved_coords and cannot be executed",
            &["unresolved_coords"],
        ));
    }
    reader.task(&path, data)?;
    Ok(json!({
        "task_json": reader.root.join(&path),
        "unresolved_coords": false,
        "declaration_scope": "declarations_only",
        "read_files": reader.read_files,
        "read_bytes": reader.read_bytes,
    }))
}

struct DeclarationReader {
    root: PathBuf,
    read_bytes: u64,
    read_files: BTreeSet<String>,
}

impl DeclarationReader {
    fn new(root: &Path) -> CliOutcome<Self> {
        let metadata = fs::symlink_metadata(root)
            .map_err(|error| invalid(root, &format!("repository metadata: {error}")))?;
        if !metadata.is_dir() || is_link(&metadata) {
            return Err(invalid(root, "repository must be a regular directory"));
        }
        let root = fs::canonicalize(root)
            .map_err(|error| invalid(root, &format!("repository path: {error}")))?;
        Ok(Self {
            root,
            read_bytes: 0,
            read_files: BTreeSet::new(),
        })
    }

    fn checked_path(&self, relative: &Path) -> CliOutcome<PathBuf> {
        validate_relative_path(relative)?;
        let mut path = self.root.clone();
        for component in relative.components() {
            path.push(component);
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| invalid(relative, &format!("declaration metadata: {error}")))?;
            if is_link(&metadata) {
                return Err(invalid(
                    relative,
                    "declaration paths must not traverse links",
                ));
            }
        }
        Ok(path)
    }

    fn read(&mut self, relative: &Path, limit: u64) -> CliOutcome<Vec<u8>> {
        let path = self.checked_path(relative)?;
        let mut file = File::open(&path)
            .map_err(|error| invalid(relative, &format!("open declaration: {error}")))?;
        let metadata = file
            .metadata()
            .map_err(|error| invalid(relative, &format!("open declaration metadata: {error}")))?;
        let limit = limit.min(MAX_DOCUMENT_BYTES);
        if !metadata.is_file() || metadata.len() > limit {
            return Err(invalid(
                relative,
                "declaration is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| invalid(relative, &format!("read declaration: {error}")))?;
        if bytes.len() as u64 > limit || self.read_bytes + bytes.len() as u64 > MAX_TOTAL_BYTES {
            return Err(invalid(relative, "declaration read budget exceeded"));
        }
        self.read_bytes += bytes.len() as u64;
        self.read_files.insert(path_text(relative));
        Ok(bytes)
    }

    fn json(&mut self, path: &Path) -> CliOutcome<Value> {
        let bytes = self.read(path, MAX_DOCUMENT_BYTES)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| invalid(path, &format!("JSON parse: {error}")))
    }

    fn validate(&mut self, path: &Path) -> CliOutcome<Value> {
        if let Some(reason) = exclusion(path) {
            return Ok(json!({"path": path_text(path), "status": "excluded", "reason": reason}));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let parent = path.parent().unwrap_or(Path::new(""));
        if !self.existing_file(path)? {
            return Err(invalid(
                path,
                "selected declaration is missing; deletion requires an explicit Git D status",
            ));
        }
        let bytes = self.read(path, MAX_DOCUMENT_BYTES)?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid(path, "declaration is not UTF-8"))?;
        let family = if name == "task.json"
            && parent
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|p| p == "operations")
        {
            let data: Value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("task JSON parse: {error}")))?;
            self.task(path, data)?;
            "operation"
        } else if name == "resources.json" && parent.file_name().is_some_and(|p| p == "operations")
        {
            let data: Value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("resource JSON parse: {error}")))?;
            validate_resource_declarations(path, &data)?;
            "resources"
        } else if name.ends_with(".pack.json") {
            actingcommand_recognition_pack::load_pack_from_json_str(text).map_err(|error| {
                declaration_parser_error(path, &error.to_string(), error.declaration_issue())
            })?;
            "recognition"
        } else if name.ends_with(".pages.json") {
            actingcommand_page_detector::load_page_set_from_json_str(text).map_err(|error| {
                declaration_parser_error(path, &error.to_string(), error.declaration_issue())
            })?;
            "pages"
        } else if name.ends_with(".navigation.json") {
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("navigation JSON parse: {error}")))?;
            validate_navigation_declarations(path, &value)?;
            DriveNavigationGraph::parse_json(text)
                .map_err(|error| invalid(path, &error.to_string()))?;
            "navigation"
        } else if name.ends_with(".projection.json") {
            ProjectionMetadata::parse(&bytes).map_err(|error| at_file(error, path))?;
            "projection"
        } else if name == "detections.json"
            && parent.file_name().is_some_and(|p| p == "env-detection")
        {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("environment JSON parse: {error}")))?;
            parse_environment_catalog_value(value)
                .map_err(|error| invalid(path, &error.to_string()))?;
            "environment"
        } else if name.starts_with("procedure-manifest.")
            && name.ends_with(".json")
            && parent.file_name().is_some_and(|p| p == "scheduling")
        {
            serde_json::from_slice::<Vec<ProcedureBindingConfigFile>>(&bytes)
                .map_err(|error| invalid(path, &format!("procedure declaration: {error}")))?;
            "procedure_manifest"
        } else if parent.file_name().is_some_and(|p| p == "scheduling") {
            let kind = match name {
                "tasks.json" => SchedulingDocumentKind::Tasks,
                "pools.json" => SchedulingDocumentKind::Pools,
                "activity.json" => SchedulingDocumentKind::Activity,
                "timeline.json" => SchedulingDocumentKind::Timeline,
                _ => return Err(invalid(path, "scheduling declaration has no mapped parser")),
            };
            validate_catalog_declaration(
                &CatalogDocumentSource::new(path_text(path), bytes.clone()),
                kind,
            )
            .map_err(|error| {
                invalid(path, &error.reason)
                    .with_details(json!({"declaration_file": path_text(path), "diagnostic": error}))
            })?;
            "scheduling"
        } else if name == "control.json" {
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("control JSON parse: {error}")))?;
            validate_control_declarations(path, &value)?;
            validate_control_declaration(value)
                .map_err(|error| invalid(path, &error.to_string()))?;
            "control"
        } else if (name == "maa-semantic-mapping.json"
            && parent.file_name().is_some_and(|p| p == "tasks"))
            || (name == "maa.tasks.json"
                && parent.file_name().is_some_and(|p| p == "upstream-sync"))
        {
            self.maa_declarations(parent.parent().unwrap_or(Path::new("")))?;
            "maa_semantic_declarations"
        } else if self.validate_task_dependency(path)? {
            "operation_json_dependency"
        } else {
            return Err(invalid(
                path,
                "program declaration family has no mapped parser",
            ));
        };
        Ok(json!({"path": path_text(path), "status": "valid", "family": family}))
    }

    fn operation_paths(&self, scope: &Path) -> CliOutcome<Vec<PathBuf>> {
        let operations = scope.join("operations");
        let absolute = self.checked_path(&operations)?;
        let mut paths = Vec::new();
        for (visited, entry) in fs::read_dir(absolute)
            .map_err(|error| invalid(&operations, &format!("read operations directory: {error}")))?
            .enumerate()
        {
            let entry = entry
                .map_err(|error| invalid(&operations, &format!("read operation entry: {error}")))?;
            if visited >= MAX_PATHS {
                return Err(invalid(
                    &operations,
                    "operation declaration selection exceeds 4096 paths",
                ));
            }
            let relative = operations.join(entry.file_name()).join("task.json");
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                invalid(&relative, &format!("operation entry metadata: {error}"))
            })?;
            if is_link(&metadata) {
                return Err(invalid(
                    &relative,
                    "operation selection must not traverse links",
                ));
            }
            if metadata.is_dir() {
                match fs::symlink_metadata(self.root.join(&relative)) {
                    Ok(_) => paths.push(relative),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(invalid(&relative, &format!("task metadata: {error}")));
                    }
                }
            }
        }
        paths.sort();
        Ok(paths)
    }

    fn maa_declarations(&mut self, scope: &Path) -> CliOutcome<()> {
        let tasks = self.operation_paths(scope)?;
        let first = tasks
            .first()
            .ok_or_else(|| invalid(scope, "MAA declarations require operation game metadata"))?;
        let data = self.json(first)?;
        self.task(first, data.clone())?;
        let game = data
            .get("game")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(first, "operation game metadata must be a string"))?;
        let mapping_path = scope.join("tasks/maa-semantic-mapping.json");
        let facts_path = scope.join("upstream-sync/maa.tasks.json");
        let mapping = self.read(&mapping_path, MAX_DOCUMENT_BYTES)?;
        let facts = self.read(&facts_path, MAX_DOCUMENT_BYTES)?;
        actingcommand_resource_tooling::validate_maa_semantic_declarations(
            &mapping_path,
            &mapping,
            &facts_path,
            &facts,
            game,
        )?;
        Ok(())
    }

    fn removal(&mut self, path: &Path) -> CliOutcome<Value> {
        // Recheck every remaining task in the same operation scope. The shared
        // declaration owner supplies all required JSON dependencies, including removed ones.
        let mut prefix = PathBuf::new();
        for component in path.components() {
            if component.as_os_str() == "operations" {
                let operations = prefix.join("operations");
                let mut checked = Vec::new();
                match fs::symlink_metadata(self.root.join(&operations)) {
                    Ok(_) => {
                        for task in self.operation_paths(&prefix)? {
                            let data = self.json(&task)?;
                            self.task(&task, data)?;
                            checked.push(path_text(&task));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(invalid(
                            path,
                            &format!("remaining operation metadata: {error}"),
                        ));
                    }
                }
                return Ok(json!({
                    "path": path_text(path), "status": "removed",
                    "reference_scope": "remaining_operation_declaration_json_dependencies",
                    "checked_declarations": checked,
                }));
            }
            prefix.push(component);
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let parent = path.parent().unwrap_or(Path::new(""));
        let mut checked = Vec::new();
        if parent.file_name().is_some_and(|name| name == "scheduling") {
            if matches!(
                name,
                "tasks.json" | "pools.json" | "activity.json" | "timeline.json"
            ) {
                let paths = ["tasks.json", "pools.json", "activity.json", "timeline.json"]
                    .map(|name| parent.join(name));
                let remaining = paths.iter().try_fold(false, |found, sibling| {
                    self.existing_file(sibling).map(|exists| found || exists)
                })?;
                if remaining {
                    for sibling in paths {
                        if !self.existing_file(&sibling)? {
                            return Err(invalid(
                                &sibling,
                                "remaining scheduling catalog requires this declaration",
                            ));
                        }
                        self.validate(&sibling)?;
                        checked.push(path_text(&sibling));
                    }
                }
            } else if !name.starts_with("procedure-manifest.") {
                return Err(invalid(
                    path,
                    "removed scheduling declaration has no mapped parser",
                ));
            }
        } else if [
            ".pack.json",
            ".pages.json",
            ".navigation.json",
            ".projection.json",
        ]
        .iter()
        .any(|suffix| name.ends_with(*suffix))
        {
            let stem = [
                ".pack.json",
                ".pages.json",
                ".navigation.json",
                ".projection.json",
            ]
            .iter()
            .find_map(|suffix| name.strip_suffix(*suffix))
            .unwrap_or("");
            let conventional = parent
                .file_name()
                .is_some_and(|part| part == "recognition" || part == "navigation");
            let scope = if conventional {
                parent.parent().unwrap_or(Path::new(""))
            } else {
                parent
            };
            let recognition_root = if conventional {
                scope.join("recognition")
            } else {
                scope.to_path_buf()
            };
            let navigation_root = if conventional {
                scope.join("navigation")
            } else {
                scope.to_path_buf()
            };
            let projection = navigation_root.join(format!("{stem}.projection.json"));
            if self.existing_file(&projection)? {
                for sibling in [
                    recognition_root.join(format!("{stem}.pack.json")),
                    recognition_root.join(format!("{stem}.pages.json")),
                    navigation_root.join(format!("{stem}.navigation.json")),
                ] {
                    if !self.existing_file(&sibling)? {
                        return Err(invalid(
                            &sibling,
                            "remaining projection declaration requires this document",
                        ));
                    }
                    self.validate(&sibling)?;
                    checked.push(path_text(&sibling));
                }
                self.validate(&projection)?;
                checked.push(path_text(&projection));
            }
        } else if (name == "maa-semantic-mapping.json"
            && parent.file_name().is_some_and(|p| p == "tasks"))
            || (name == "maa.tasks.json"
                && parent.file_name().is_some_and(|p| p == "upstream-sync"))
        {
            let scope = parent.parent().unwrap_or(Path::new(""));
            let mapping = scope.join("tasks/maa-semantic-mapping.json");
            let facts = scope.join("upstream-sync/maa.tasks.json");
            if self.existing_file(&mapping)? || self.existing_file(&facts)? {
                self.maa_declarations(scope)?;
                checked.extend([path_text(&mapping), path_text(&facts)]);
            }
        } else if !(name == "detections.json"
            && parent.file_name().is_some_and(|p| p == "env-detection"))
            && name != "control.json"
        {
            return Err(invalid(
                path,
                "removed declaration has no mapped current-reference check",
            ));
        }
        Ok(json!({
            "path": path_text(path), "status": "removed",
            "reference_scope": "current_local_declaration_dependencies",
            "checked_declarations": checked,
        }))
    }

    fn existing_file(&self, relative: &Path) -> CliOutcome<bool> {
        validate_relative_path(relative)?;
        let mut path = self.root.clone();
        for component in relative.components() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if is_link(&metadata) => {
                    return Err(invalid(
                        relative,
                        "declaration paths must not traverse links",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(invalid(relative, &format!("declaration metadata: {error}")));
                }
            }
        }
        Ok(true)
    }

    fn validate_task_dependency(&mut self, path: &Path) -> CliOutcome<bool> {
        for ancestor in path.ancestors().skip(1) {
            if ancestor
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|part| part == "operations")
            {
                let task = ancestor.join("task.json");
                let data = self.json(&task)?;
                let bundle = Bundle {
                    task_id: data
                        .get("task_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    dir: ancestor.to_path_buf(),
                    data: data.clone(),
                };
                if declaration_file_requests(std::slice::from_ref(&bundle))?.contains_key(path) {
                    self.task(&task, data)?;
                    return Ok(true);
                }
                return Ok(false);
            }
        }
        Ok(false)
    }

    fn task(&mut self, path: &Path, data: Value) -> CliOutcome<()> {
        let dir = path
            .parent()
            .ok_or_else(|| invalid(path, "missing task directory"))?;
        let bundle = Bundle {
            task_id: data
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            dir: dir.to_path_buf(),
            data,
        };
        let requests = declaration_file_requests(std::slice::from_ref(&bundle))?;
        let mut files = BTreeMap::new();
        for (dependency, read) in requests {
            let SourceRead::BoundedBytes(limit) = read else {
                return Err(invalid(
                    &dependency,
                    "declaration parser requested a non-JSON read",
                ));
            };
            let bytes = self.read(&dependency, limit)?;
            files.insert(
                dependency,
                SourceFile {
                    is_file: true,
                    length: Ok(bytes.len() as u64),
                    bytes: Ok(bytes),
                },
            );
        }
        validate_bundle_declarations(
            &bundle,
            &ConversionFiles {
                files: Arc::new(files),
                projection_bytes: Ok(None),
                projection_exists: Ok(false),
            },
        )?;
        let resource_path = dir
            .parent()
            .ok_or_else(|| invalid(path, "missing operations directory"))?
            .join("resources.json");
        let resources = self.json(&resource_path)?;
        validate_resource_declarations(&resource_path, &resources)
    }
}

fn exclusion(path: &Path) -> Option<&'static str> {
    let text = path_text(path);
    if text.starts_with(".github/") {
        return Some("workflow_configuration");
    }
    if text == "manifest.yaml" {
        return Some("repository_provenance_manifest");
    }
    if text.starts_with("upstream-derived/") || text.starts_with("packages/") {
        return Some("upstream_or_archived_material");
    }
    if ["ours/art/", "ours/characters/", "ours/materials/"]
        .iter()
        .any(|prefix| text.starts_with(*prefix))
    {
        return Some("art_catalog_material");
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "SOURCE.json"
            | "REUSED.json"
            | "task.src.json"
            | "operations.index.json"
            | "operations.primitives.json"
            | "resources.safety.json"
            | "resources.migration.json"
            | "task-annotations.json"
            | "task-catalog.json"
            | "home-facts.template.json"
    ) || text.contains("/clicks/")
        || text.contains("/components/")
        || text.contains("/preparation/")
        || text.starts_with("ours/recovery/")
        || text.starts_with("ours/recognition/overrides/")
    {
        return Some("authoring_or_reference_material");
    }
    if !matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("json" | "yaml" | "yml")
    ) {
        return Some("not_a_declaration_document");
    }
    None
}

fn validate_relative_path(path: &Path) -> CliOutcome<()> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid(path, "path is not UTF-8"))?;
    if text.is_empty()
        || text.len() > MAX_PATH_BYTES
        || text.contains(':')
        || (!cfg!(windows) && text.contains('\\'))
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid(
            path,
            "path must use repository-relative normal components",
        ));
    }
    Ok(())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn at_file(mut error: CliError, path: &Path) -> CliError {
    error.message = format!("{}: {}", path_text(path), error.message);
    let mut details = error.details.take().unwrap_or_else(|| json!({}));
    if let Some(object) = details.as_object_mut() {
        object.insert("declaration_file".to_string(), json!(path_text(path)));
    } else {
        details = json!({"declaration_file": path_text(path), "parser_details": details});
    }
    error.details = Some(details);
    error
}

fn declaration_parser_error(
    path: &Path,
    reason: &str,
    issue: Option<&ResourceDeclarationIssue>,
) -> CliError {
    let failure = invalid(path, reason);
    if let Some(issue) = issue {
        let mut issue = issue.clone();
        issue.declaration_file = path_text(path);
        failure.with_details(json!(issue))
    } else {
        failure
    }
}

fn invalid(path: &Path, reason: &str) -> CliError {
    CliError::new(
        LabErrorClass::UsageValidation,
        "resource_declaration_invalid",
        format!(
            "{}: {}",
            path_text(path),
            reason.chars().take(1024).collect::<String>()
        ),
        &[],
    )
    .with_details(json!({"declaration_file": path_text(path)}))
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
