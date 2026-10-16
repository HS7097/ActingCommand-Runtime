// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only adapters from repository paths to the production declaration parsers.

use crate::codes::ActinglabCode;
use crate::{CliError, CliOutcome, FlagArgs};
use actingcommand_contract::{
    BundleMaintenance, LabErrorClass, ResourceAdjustmentDeclaration, ResourceDeclarationIssue,
    ResourceReadingDeclaration, STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE,
    STANDARD_PACK_PAGES_FILE, page_projection::ProjectionMetadata, parse_standard_pack_interrupts,
    parse_standard_pack_navigation, parse_standard_pack_pages,
    resource_declaration::ProcedureBindingConfigFile, validate_bundle_maintenance_declarations,
};
use actingcommand_lab::{parse_environment_catalog_value, validate_control_declaration};
use actingcommand_pack_containment::source::{
    Bundle, ParseFiles, SourceFile, SourceRead, declaration_file_requests,
    validate_bundle_declarations, validate_control_declarations, validate_navigation_declarations,
    validate_resource_declarations,
};
use actingcommand_policy::{
    CatalogDocumentSource, ObservationRef, PoolValueSource, PoolsDocument, SchedulingDocumentKind,
    validate_catalog_declaration,
};
use actingcommand_resource_tooling::{DECLARATION_EXCLUSIONS_FILE, DeclarationExclusions};
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
/// Workflow #390 nine6: the repository entries the operation scan of a pools document lists.
const MAX_SCAN_ENTRIES: usize = 262_144;

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
    // Workflow #390 nine6: a warning leaves the validation valid; the developer line makes it
    // visible beside the human summary as well.
    let warnings = reader
        .pool_warnings
        .iter()
        .map(PoolProducerWarning::answer)
        .collect::<Vec<_>>();
    for warning in &reader.pool_warnings {
        eprintln!(
            "WARN actinglab: {} path={} json_pointer={} fact_key={}",
            ActinglabCode::ResourcePoolProducerMissing.as_str(),
            warning.path,
            warning.json_pointer,
            warning.fact_key
        );
    }
    Ok(json!({
        "schema_version": "actinglab.resource-declarations.v1",
        "scope": "declarations_only",
        "repo": reader.root,
        "status": "valid",
        "entries": entries,
        "warnings": warnings,
        "read_files": reader.read_files,
        "read_bytes": reader.read_bytes,
    }))
}

/// Workflow #390 nine6: one `resource_pool_producer_missing` warning, a `ledger_fact` pool of
/// the pools document `path` that nothing in the repository publishes.
struct PoolProducerWarning {
    path: String,
    json_pointer: String,
    fact_key: String,
}

impl PoolProducerWarning {
    /// `{code, path, json_pointer, fact_key}`, one element of the answer's `warnings`.
    fn answer(&self) -> Value {
        json!({
            "code": ActinglabCode::ResourcePoolProducerMissing.as_str(),
            "path": self.path,
            "json_pointer": self.json_pointer,
            "fact_key": self.fact_key,
        })
    }
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
    /// Workflow #396 TOOL-2: the repository's `declaration-exclusions.json`, read once, when a
    /// path without a mapped parser first needs it.
    exclusions: Option<DeclarationExclusions>,
    /// Workflow #390 nine6: the instance fact keys the repository's operations publish, read
    /// once, when a pools document with a `ledger_fact` pool first needs them.
    published_keys: Option<BTreeSet<String>>,
    /// The pools documents already checked for producers in this run, by path.
    checked_pools: BTreeSet<String>,
    pool_warnings: Vec<PoolProducerWarning>,
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
            exclusions: None,
            published_keys: None,
            checked_pools: BTreeSet::new(),
            pool_warnings: Vec::new(),
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
                _ => return self.unmapped(path, "scheduling declaration has no mapped parser"),
            };
            validate_catalog_declaration(
                &CatalogDocumentSource::new(path_text(path), bytes.clone()),
                kind,
            )
            .map_err(|error| {
                invalid(path, &error.reason)
                    .with_details(json!({"declaration_file": path_text(path), "diagnostic": error}))
            })?;
            if kind == SchedulingDocumentKind::Pools {
                self.check_pool_producers(path, &bytes)?;
            }
            "scheduling"
        } else if name == "control.json" {
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| invalid(path, &format!("control JSON parse: {error}")))?;
            validate_control_declarations(path, &value)?;
            validate_control_declaration(value)
                .map_err(|error| invalid(path, &error.to_string()))?;
            "control"
        } else if is_applications_table(path) {
            self.applications(path, &bytes)?;
            "applications"
        } else if path == Path::new("maintenance.json") {
            let maintenance = serde_json::from_slice::<Vec<BundleMaintenance>>(&bytes)
                .map_err(|error| invalid(path, &format!("maintenance declaration: {error}")))?;
            validate_bundle_maintenance_declarations(&maintenance)
                .map_err(|error| invalid(path, &error.to_string()))?;
            "maintenance"
        } else if path == Path::new(DECLARATION_EXCLUSIONS_FILE) {
            DeclarationExclusions::parse(&bytes)?;
            "declaration_exclusions"
        } else if path == Path::new(STANDARD_PACK_PAGES_FILE) {
            // Workflow #373 NAV-6: the standard-pack page list, with the Runtime's own checks;
            // the bundle generator checks the packages it names (M-373 §3.9).
            parse_standard_pack_pages(&bytes, None)
                .map_err(|issue| invalid(path, &issue.to_string()))?;
            "standard_pack_pages"
        } else if path == Path::new(STANDARD_PACK_INTERRUPTS_FILE) {
            // Workflow #373 NAV-6: the shared interrupt pages, against the page list beside them.
            let pages_path = Path::new(STANDARD_PACK_PAGES_FILE);
            let pages = self.read(pages_path, MAX_DOCUMENT_BYTES)?;
            let pages = parse_standard_pack_pages(&pages, None)
                .map_err(|issue| invalid(pages_path, &issue.to_string()))?;
            parse_standard_pack_interrupts(&bytes, Some(&pages.game), &pages)
                .map_err(|issue| invalid(path, &issue.to_string()))?;
            "standard_pack_interrupts"
        } else if path == Path::new(STANDARD_PACK_NAVIGATION_FILE) {
            // Workflow #373 NAV-5: the edges and the recovery chain, against the page list beside
            // them; the bundle generator checks the packages they name (M-373 §3.9).
            let pages_path = Path::new(STANDARD_PACK_PAGES_FILE);
            let pages = self.read(pages_path, MAX_DOCUMENT_BYTES)?;
            let pages = parse_standard_pack_pages(&pages, None)
                .map_err(|issue| invalid(pages_path, &issue.to_string()))?;
            parse_standard_pack_navigation(&bytes, Some(&pages.game), &pages)
                .map_err(|issue| invalid(path, &issue.to_string()))?;
            "standard_pack_navigation"
        } else if (name == "maa-semantic-mapping.json"
            && parent.file_name().is_some_and(|p| p == "tasks"))
            || (name == "maa.tasks.json"
                && parent.file_name().is_some_and(|p| p == "upstream-sync"))
        {
            self.maa_declarations(parent.parent().unwrap_or(Path::new("")))?;
            "maa_semantic_declarations"
        } else if let Some(family) = self.validate_task_dependency(path)? {
            family
        } else {
            return self.unmapped(path, "program declaration family has no mapped parser");
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

    /// Checks the repository-root application table against the repository manifest.
    fn applications(&mut self, path: &Path, bytes: &[u8]) -> CliOutcome<()> {
        let table: ApplicationTable = serde_json::from_slice(bytes)
            .map_err(|error| invalid(path, &format!("applications declaration: {error}")))?;
        if table.schema_version != APPLICATIONS_SCHEMA {
            return Err(invalid(
                path,
                &format!("schema_version must be {APPLICATIONS_SCHEMA}"),
            ));
        }
        let manifest = self
            .read(Path::new("manifest.yaml"), MAX_DOCUMENT_BYTES)
            .map_err(|error| {
                invalid(
                    path,
                    &format!(
                        "repository manifest.yaml is missing or unreadable: {}",
                        error.message
                    ),
                )
            })?;
        let (game, supported) = std::str::from_utf8(&manifest)
            .map_err(|_| "is not UTF-8".to_string())
            .and_then(manifest_identity)
            .map_err(|reason| invalid(path, &format!("repository manifest.yaml {reason}")))?;
        if table.game != game {
            return Err(invalid(
                path,
                &format!(
                    "game {:?} does not match repository manifest.yaml game {game:?}",
                    table.game
                ),
            ));
        }
        if let Some(label) = &table.label {
            let text = label.trim();
            if text.is_empty() || text.len() > 128 || text.chars().any(char::is_control) {
                return Err(invalid(
                    path,
                    &format!(
                        "label {label:?} must be 1-128 bytes of text without control characters"
                    ),
                ));
            }
        }
        if table.servers.is_empty() {
            return Err(invalid(path, "servers must not be empty"));
        }
        let mut owners = BTreeMap::new();
        for (server, entry) in &table.servers {
            if !supported.contains(server) {
                return Err(invalid(
                    path,
                    &format!(
                        "server {server:?} is not listed in repository manifest.yaml supported_servers"
                    ),
                ));
            }
            if entry.label.is_empty()
                || entry.label.len() > 64
                || entry.label.chars().any(char::is_control)
            {
                return Err(invalid(
                    path,
                    &format!(
                        "servers.{server}.label must be 1-64 bytes without control characters"
                    ),
                ));
            }
            if !is_application_id(&entry.application_id) {
                return Err(invalid(
                    path,
                    &format!(
                        "servers.{server}.application_id {:?} is not a dotted Android application id of at most 255 bytes",
                        entry.application_id
                    ),
                ));
            }
            if let Some(package) = &entry.default_package_id
                && !is_package_id(package)
            {
                return Err(invalid(
                    path,
                    &format!(
                        "servers.{server}.default_package_id {package:?} must be 1-128 bytes matching ^[a-z0-9]+(\\.[a-z0-9_]+)+$"
                    ),
                ));
            }
            if let Some(previous) = owners.insert(entry.application_id.as_str(), server) {
                return Err(invalid(
                    path,
                    &format!(
                        "servers {previous:?} and {server:?} share application_id {:?}",
                        entry.application_id
                    ),
                ));
            }
        }
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
                return self.unmapped(path, "removed scheduling declaration has no mapped parser");
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
            && !is_applications_table(path)
            && path != Path::new(DECLARATION_EXCLUSIONS_FILE)
        {
            return self.unmapped(
                path,
                "removed declaration has no mapped current-reference check",
            );
        }
        Ok(json!({
            "path": path_text(path), "status": "removed",
            "reference_scope": "current_local_declaration_dependencies",
            "checked_declarations": checked,
        }))
    }

    /// Workflow #396 TOOL-2: a path no declaration parser is mapped to is `excluded` when it
    /// lies inside a directory of the repository's `declaration-exclusions.json`, and refused
    /// with `reason` otherwise. A repository without the file declares no directory; a file that
    /// does not parse is refused.
    fn unmapped(&mut self, path: &Path, reason: &str) -> CliOutcome<Value> {
        let exclusions = match self.exclusions.take() {
            Some(exclusions) => exclusions,
            None => {
                let file = Path::new(DECLARATION_EXCLUSIONS_FILE);
                if self.existing_file(file)? {
                    let bytes = self.read(file, MAX_DOCUMENT_BYTES)?;
                    DeclarationExclusions::parse(&bytes)?
                } else {
                    DeclarationExclusions::default()
                }
            }
        };
        let covered = exclusions.covers(path);
        self.exclusions = Some(exclusions);
        if covered {
            Ok(json!({
                "path": path_text(path), "status": "excluded",
                "reason": "repository_declared_exclusion",
            }))
        } else {
            Err(invalid(path, reason))
        }
    }

    /// Workflow #390 nine6 (Alice's nine additions of 2026-10-10, item 6): a `ledger_fact` pool
    /// of the valid pools document `path` that nothing in the repository publishes gets a
    /// `resource_pool_producer_missing` warning, and the document stays valid. A reading mark or
    /// an adjustment publishes an instance fact of its key (`contracts/resource-readings.md`,
    /// "Host"), and a live pool reads its key in its own scope, so a pool is published only when
    /// it is of instance scope and an operation of the repository writes its key. A static
    /// snapshot pool takes the configuration's value and is not checked.
    fn check_pool_producers(&mut self, path: &Path, bytes: &[u8]) -> CliOutcome<()> {
        let document_path = path_text(path);
        if !self.checked_pools.insert(document_path.clone()) {
            return Ok(());
        }
        let document: PoolsDocument = serde_json::from_slice(bytes)
            .map_err(|error| invalid(path, &format!("pools declaration: {error}")))?;
        let live = document
            .pools
            .iter()
            .enumerate()
            .filter_map(
                |(index, pool)| match (&pool.value_source, &pool.observation) {
                    (PoolValueSource::LedgerFact { .. }, ObservationRef::Fact { fact_key }) => {
                        Some((index, &pool.scope, fact_key))
                    }
                    _ => None,
                },
            )
            .collect::<Vec<_>>();
        if live.is_empty() {
            return Ok(());
        }
        let published = match self.published_keys.take() {
            Some(keys) => keys,
            None => self.repository_published_keys()?,
        };
        for (index, _scope, fact_key) in live {
            if !published.contains(fact_key.as_str()) {
                self.pool_warnings.push(PoolProducerWarning {
                    path: document_path.clone(),
                    json_pointer: format!("/pools/{index}"),
                    fact_key: fact_key.clone(),
                });
            }
        }
        self.published_keys = Some(published);
        Ok(())
    }

    /// Workflow #390 nine6: every instance fact key an operation of the repository writes: each
    /// `resource_readings` mark's keys (`fact_key`, and `capacity_fact_key` when it declares one)
    /// and each `resource_adjustments` entry's `fact_key`, read with the contract's declarations.
    fn repository_published_keys(&mut self) -> CliOutcome<BTreeSet<String>> {
        let mut keys = BTreeSet::new();
        for task in self.repository_operations()? {
            let data = self.json(&task)?;
            if let Some(readings) = data.get("resource_readings") {
                let readings =
                    <Vec<ResourceReadingDeclaration> as serde::Deserialize>::deserialize(readings)
                        .map_err(|error| invalid(&task, &format!("resource_readings: {error}")))?;
                for reading in &readings {
                    keys.extend(reading.fact_keys().take(1).map(str::to_owned));
                }
            }
            if let Some(adjustments) = data.get("resource_adjustments") {
                let adjustments =
                    <Vec<ResourceAdjustmentDeclaration> as serde::Deserialize>::deserialize(
                        adjustments,
                    )
                    .map_err(|error| invalid(&task, &format!("resource_adjustments: {error}")))?;
                keys.extend(
                    adjustments
                        .into_iter()
                        .map(|adjustment| adjustment.fact_key)
                        .filter(|_| false),
                );
            }
        }
        Ok(keys)
    }

    /// Workflow #390 nine6: every operation `task.json` of the repository, in path order: a file
    /// `<directory>/operations/<task>/task.json` at any depth, as the validator recognizes an
    /// operation, outside Git's own `.git` and the excluded material (`exclusion`: upstream and
    /// archived material, art catalogs, authoring and reference files). A link is refused, as
    /// every declaration path refuses one.
    fn repository_operations(&self) -> CliOutcome<Vec<PathBuf>> {
        let mut directories = vec![PathBuf::new()];
        let mut operations = Vec::new();
        let mut listed = 0_usize;
        while let Some(directory) = directories.pop() {
            let entries = fs::read_dir(self.root.join(&directory)).map_err(|error| {
                invalid(&directory, &format!("read repository directory: {error}"))
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    invalid(&directory, &format!("read repository entry: {error}"))
                })?;
                listed += 1;
                if listed > MAX_SCAN_ENTRIES {
                    return Err(invalid(
                        &directory,
                        "operation scan exceeds 262144 repository entries",
                    ));
                }
                let name = entry.file_name();
                if name.as_os_str() == ".git" {
                    continue;
                }
                let path = directory.join(&name);
                let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                    invalid(&path, &format!("repository entry metadata: {error}"))
                })?;
                if is_link(&metadata) {
                    return Err(invalid(&path, "operation scan must not traverse links"));
                }
                if metadata.is_dir() {
                    // Every path below an excluded directory is excluded material too.
                    if exclusion(&path.join("task.json")).is_none() {
                        directories.push(path);
                    }
                } else if name.as_os_str() == "task.json"
                    && directory
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|parent| parent == "operations")
                    && exclusion(&path).is_none()
                {
                    operations.push(path);
                }
            }
        }
        operations.sort();
        Ok(operations)
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

    /// The family of a task-local JSON dependency, validated through its task: a select step's
    /// `selection_policy` document or an `operation_json_dependency` (truth set, dictionary);
    /// `None` when no task requests the path.
    fn validate_task_dependency(&mut self, path: &Path) -> CliOutcome<Option<&'static str>> {
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
                let family =
                    match declaration_file_requests(std::slice::from_ref(&bundle))?.get(path) {
                        Some(SourceRead::SelectionPolicy(_)) => "selection_policy",
                        Some(_) => "operation_json_dependency",
                        None => return Ok(None),
                    };
                self.task(&task, data)?;
                return Ok(Some(family));
            }
        }
        Ok(None)
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
            let (SourceRead::BoundedBytes(limit) | SourceRead::SelectionPolicy(limit)) = read
            else {
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
            &ParseFiles {
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
    if [
        "ours/art/",
        "ours/characters/",
        "ours/equipment/",
        "ours/materials/",
    ]
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

const APPLICATIONS_SCHEMA: &str = "actingcommand.applications.v1";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationTable {
    schema_version: String,
    game: String,
    /// The game's display name for people. Absent stays `None`; a present value must be a
    /// string (explicit `null` fails).
    #[serde(default, deserialize_with = "present_string")]
    label: Option<String>,
    servers: BTreeMap<String, ApplicationEntry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationEntry {
    application_id: String,
    label: String,
    /// Absent stays `None`; a present value must be a string (explicit `null` fails).
    #[serde(default, deserialize_with = "present_string")]
    default_package_id: Option<String>,
}

fn present_string<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(value).map(Some)
}

/// Only the table directly under the repository root belongs to the applications family.
fn is_applications_table(path: &Path) -> bool {
    path == Path::new("applications.json")
}

/// `^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$`, at most 255 bytes.
fn is_application_id(id: &str) -> bool {
    id.len() <= 255
        && id.contains('.')
        && id.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

/// `^[a-z0-9]+(\.[a-z0-9_]+)+$`, at most 128 bytes.
fn is_package_id(id: &str) -> bool {
    let plain = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    id.len() <= 128
        && id.split_once('.').is_some_and(|(first, rest)| {
            !first.is_empty()
                && first.chars().all(plain)
                && rest.split('.').all(|segment| {
                    !segment.is_empty()
                        && segment
                            .chars()
                            .all(|character| plain(character) || character == '_')
                })
        })
}

/// Reads the top-level `game` scalar and `supported_servers` list of the repository manifest.
/// Only plain block or flow forms are understood; any other shape fails instead of being skipped.
fn manifest_identity(text: &str) -> Result<(String, BTreeSet<String>), String> {
    let nested = |line: &str| line.trim().is_empty() || line.starts_with([' ', '\t', '#', '-']);
    let mut game = None;
    let mut servers = None;
    let mut lines = text.trim_start_matches('\u{feff}').lines().peekable();
    while let Some(line) = lines.next() {
        if nested(line) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = plain_scalar(value);
        match key.trim_end() {
            "game" => {
                if value.is_empty() || game.replace(value.to_string()).is_some() {
                    return Err("must declare exactly one non-empty top-level game".to_string());
                }
            }
            "supported_servers" => {
                let items = if let Some(flow) = value
                    .strip_prefix('[')
                    .and_then(|flow| flow.strip_suffix(']'))
                {
                    flow.split(',')
                        .filter(|item| !item.trim().is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                } else if value.is_empty() {
                    let mut block = Vec::new();
                    while let Some(item) = lines.next_if(|next| nested(next)) {
                        let item = item.trim();
                        if item.is_empty() || item.starts_with('#') {
                            continue;
                        }
                        let entry = item.strip_prefix("- ").ok_or_else(|| {
                            "supported_servers must be a list of plain scalars".to_string()
                        })?;
                        block.push(entry.to_string());
                    }
                    block
                } else {
                    return Err("supported_servers must be a list".to_string());
                };
                let mut list = BTreeSet::new();
                for item in &items {
                    let item = plain_scalar(item);
                    if item.is_empty() || item.ends_with(':') || item.contains(": ") {
                        return Err("supported_servers must be a list of plain scalars".to_string());
                    }
                    list.insert(item.to_string());
                }
                if servers.replace(list).is_some() {
                    return Err("must declare exactly one top-level supported_servers".to_string());
                }
            }
            _ => {}
        }
    }
    match (game, servers) {
        (Some(game), Some(servers)) => Ok((game, servers)),
        (None, _) => Err("has no top-level game".to_string()),
        (_, None) => Err("has no top-level supported_servers".to_string()),
    }
}

/// Drops a trailing comment and one pair of surrounding quotes from a YAML scalar.
fn plain_scalar(value: &str) -> &str {
    let value = value.trim();
    let end = value
        .char_indices()
        .find(|&(index, character)| {
            character == '#' && (index == 0 || value[..index].ends_with(char::is_whitespace))
        })
        .map_or(value.len(), |(index, _)| index);
    let value = value[..end].trim_end();
    ['"', '\'']
        .iter()
        .find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(value)
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
