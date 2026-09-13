// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ResourceConvertRequest, ResourceConvertResponse, maa_task_graph};
use actingcommand_contract::{LabError as CliError, LabResult as CliOutcome};
pub(crate) use actingcommand_pack_containment::source::validate_phases_bundle;
use actingcommand_pack_containment::source::{
    self, ConversionFiles, SourceFile, SourceRead, canonical_resource_identifier,
    first_server_scope, required_string, resource_ids, string_field,
};
pub use actingcommand_pack_containment::source::{
    Bundle, ConvertOutputs, canonical_game, canonical_locale, canonical_server,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAA_SEMANTIC_MAPPING_PATH: &str = "tasks/maa-semantic-mapping.json";
const MAA_TASK_FACTS_PATH: &str = "upstream-sync/maa.tasks.json";
const MAA_TASK_FACTS_DECLARED_PATH: &str = "ours/upstream-sync/maa.tasks.json";
const MAA_SEMANTIC_MAPPING_SCHEMA: &str = "actingcommand.maa-semantic-mapping.v1";
const MAA_TASK_FACTS_SCHEMA: &str = "actingcommand.maa-task-facts-set.v1";
const MAA_SEMANTIC_ROLES: [&str; 5] = [
    "page_anchor",
    "page_transition",
    "page_operation",
    "observation",
    "topology",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaaSemanticMappingDocument {
    schema_version: String,
    facts_container: MaaFactsContainerBinding,
    mappings: Vec<MaaSemanticMappingRow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaaFactsContainerBinding {
    path: String,
    sha256: String,
    data_schema_version: String,
    task_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaaSemanticMappingRow {
    source_task_id: String,
    product_heading: String,
    page_id: String,
    role: String,
}

#[derive(Debug, Deserialize)]
struct MaaTaskFactsEnvelope {
    data: MaaTaskFactsData,
}

#[derive(Debug, Deserialize)]
struct MaaTaskFactsData {
    schema_version: String,
    tasks: Vec<MaaTaskFactsEntry>,
}

#[derive(Debug, Deserialize)]
struct MaaTaskFactsEntry {
    task_id: MaaTaskFactsId,
}

#[derive(Debug, Deserialize)]
struct MaaTaskFactsId {
    value: String,
}

pub fn resource_convert(request: ResourceConvertRequest) -> CliOutcome<ResourceConvertResponse> {
    let resource_root = resolve_resource_root(&request.repo);
    let repo = &resource_root.root;
    let game_override = request.game.as_deref().map(canonical_game).transpose()?;
    let mut converter = OperationConverter::load(
        repo,
        game_override.as_deref(),
        request.server.as_deref(),
        request.locale.as_deref(),
    )?;
    let maa_semantic_mappings = admit_maa_semantic_mapping(repo, &converter.game)?;
    let maa_tasks_root = request.maa_tasks_root;
    if let Some(tasks_root) = maa_tasks_root.as_deref() {
        converter.load_maa_task_overlays(tasks_root)?;
    }
    let outputs = converter.build_all()?;
    let dry_run = request.dry_run;
    if !dry_run {
        write_outputs(&outputs, repo)?;
    }
    let maa_compiled_tasks = maa_tasks_root
        .as_ref()
        .map(|_| converter.maa_task_overlays.len());
    Ok(ResourceConvertResponse {
        repo: resource_root.input.display().to_string(),
        resource_root: repo.display().to_string(),
        resource_layout: resource_root.layout.to_string(),
        game: converter.game,
        server: converter.server,
        locale: converter.locale,
        dry_run,
        maa_semantic_mappings,
        bundles: converter.bundles.len(),
        targets: outputs
            .pack
            .get("targets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        pages: outputs
            .pages
            .get("pages")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        edges: outputs
            .navigation
            .get("navigation")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        page_operations: outputs
            .navigation
            .get("page_operations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        index_tasks: outputs
            .index
            .get("operations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        primitives: outputs
            .primitives
            .get("primitives")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        status: if dry_run { "validated" } else { "written" }.to_string(),
        source_mode: maa_tasks_root.as_ref().map(|_| "maa_tasks".to_string()),
        maa_tasks_root: maa_tasks_root.map(|path| path.display().to_string()),
        maa_compiled_tasks,
    })
}

fn admit_maa_semantic_mapping(root: &Path, game: &str) -> CliOutcome<usize> {
    let mapping_path = root.join(MAA_SEMANTIC_MAPPING_PATH);
    let facts_path = root.join(MAA_TASK_FACTS_PATH);
    let read_source = |path: &Path| match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError::package_invalid(format!(
            "failed to read {}: {error}",
            path.display()
        ))),
    };
    let mapping_bytes = read_source(&mapping_path)?;
    let facts_bytes = read_source(&facts_path)?;
    let (mapping_bytes, facts_bytes) = match (mapping_bytes, facts_bytes) {
        (None, None) => return Ok(0),
        (Some(mapping_bytes), Some(facts_bytes)) => (mapping_bytes, facts_bytes),
        (None, Some(_)) => {
            return Err(CliError::package_invalid(format!(
                "canonical MAA pair requires {}",
                mapping_path.display()
            )));
        }
        (Some(_), None) => {
            return Err(CliError::package_invalid(format!(
                "canonical MAA pair requires {}",
                facts_path.display()
            )));
        }
    };
    validate_maa_semantic_declarations(
        &mapping_path,
        &mapping_bytes,
        &facts_path,
        &facts_bytes,
        game,
    )
}

/// Validate the same MAA declaration pair without reading assets or converting resources.
pub fn validate_maa_semantic_declarations(
    mapping_path: &Path,
    mapping_bytes: &[u8],
    facts_path: &Path,
    facts_bytes: &[u8],
    game: &str,
) -> CliOutcome<usize> {
    let mapping: MaaSemanticMappingDocument =
        serde_json::from_slice(mapping_bytes).map_err(|error| {
            CliError::package_invalid(format!(
                "failed to parse {}: {error}",
                mapping_path.display()
            ))
        })?;
    if mapping.schema_version != MAA_SEMANTIC_MAPPING_SCHEMA {
        return Err(CliError::package_invalid(format!(
            "{}: mapping schema must be {MAA_SEMANTIC_MAPPING_SCHEMA}",
            mapping_path.display()
        )));
    }
    if mapping.facts_container.path != MAA_TASK_FACTS_DECLARED_PATH {
        return Err(CliError::package_invalid(format!(
            "{}: facts_container.path must be {MAA_TASK_FACTS_DECLARED_PATH}",
            mapping_path.display()
        )));
    }
    if mapping.facts_container.data_schema_version != MAA_TASK_FACTS_SCHEMA {
        return Err(CliError::package_invalid(format!(
            "{}: facts_container data schema must be {MAA_TASK_FACTS_SCHEMA}",
            mapping_path.display()
        )));
    }
    let task_count = mapping.facts_container.task_count;
    if task_count == 0 || task_count > maa_task_graph::MAX_MAA_TASK_FACT_SELECTIONS {
        return Err(CliError::package_invalid(format!(
            "{}: facts_container.task_count must be within 1..={}",
            mapping_path.display(),
            maa_task_graph::MAX_MAA_TASK_FACT_SELECTIONS
        )));
    }
    if mapping.mappings.len() != task_count {
        return Err(CliError::package_invalid(format!(
            "{}: mapping row count {} does not match facts_container.task_count {task_count}",
            mapping_path.display(),
            mapping.mappings.len()
        )));
    }

    let actual_sha256 = format!("{:x}", Sha256::digest(facts_bytes));
    if actual_sha256 != mapping.facts_container.sha256 {
        return Err(CliError::package_invalid(format!(
            "{}: A1 facts container SHA-256 mismatch",
            facts_path.display()
        )));
    }
    let facts: MaaTaskFactsEnvelope = serde_json::from_slice(facts_bytes).map_err(|error| {
        CliError::package_invalid(format!("failed to parse {}: {error}", facts_path.display()))
    })?;
    if facts.data.schema_version != MAA_TASK_FACTS_SCHEMA {
        return Err(CliError::package_invalid(format!(
            "{}: facts data schema must be {MAA_TASK_FACTS_SCHEMA}",
            facts_path.display()
        )));
    }
    if facts.data.tasks.len() != mapping.facts_container.task_count {
        return Err(CliError::package_invalid(format!(
            "{}: actual task count {} does not match facts_container.task_count {}",
            facts_path.display(),
            facts.data.tasks.len(),
            mapping.facts_container.task_count
        )));
    }

    let mut fact_ids = Vec::with_capacity(facts.data.tasks.len());
    let mut unique_fact_ids = HashSet::with_capacity(facts.data.tasks.len());
    for task in &facts.data.tasks {
        let task_id = task.task_id.value.as_str();
        if task_id.is_empty() {
            return Err(CliError::package_invalid(format!(
                "{}: A1 source_task_id must be non-empty",
                facts_path.display()
            )));
        }
        if !unique_fact_ids.insert(task_id) {
            return Err(CliError::package_invalid(format!(
                "{}: duplicate A1 source_task_id '{task_id}'",
                facts_path.display()
            )));
        }
        fact_ids.push(task_id);
    }

    let mut mapping_ids = HashSet::with_capacity(mapping.mappings.len());
    for row in &mapping.mappings {
        if !mapping_ids.insert(row.source_task_id.as_str()) {
            return Err(CliError::package_invalid(format!(
                "{}: duplicate source_task_id '{}'",
                mapping_path.display(),
                row.source_task_id
            )));
        }
        if !unique_fact_ids.contains(row.source_task_id.as_str()) {
            return Err(CliError::package_invalid(format!(
                "{}: unknown source_task_id '{}'",
                mapping_path.display(),
                row.source_task_id
            )));
        }
        if !canonical_resource_identifier("mapping product heading", &row.product_heading)
            .is_ok_and(|value| value == row.product_heading)
        {
            return Err(CliError::package_invalid(format!(
                "{}: invalid product_heading '{}'",
                mapping_path.display(),
                row.product_heading
            )));
        }
        if !MAA_SEMANTIC_ROLES.contains(&row.role.as_str()) {
            return Err(CliError::package_invalid(format!(
                "{}: unknown role '{}'",
                mapping_path.display(),
                row.role
            )));
        }
        if !is_exact_mapping_page_id(&row.page_id, game) {
            return Err(CliError::package_invalid(format!(
                "{}: invalid page_id '{}' for converter game '{game}'",
                mapping_path.display(),
                row.page_id
            )));
        }
    }

    if fact_ids
        .iter()
        .any(|task_id| !mapping_ids.contains(task_id))
    {
        return Err(CliError::package_invalid(format!(
            "{}: mapping is missing an A1 source_task_id",
            mapping_path.display()
        )));
    }
    if let Some((index, (mapping_row, fact_id))) = mapping
        .mappings
        .iter()
        .zip(fact_ids.iter())
        .enumerate()
        .find(|(_, (mapping_row, fact_id))| mapping_row.source_task_id.as_str() != **fact_id)
    {
        return Err(CliError::package_invalid(format!(
            "{}: source_task_id ordinal order mismatch at row {index}: found '{}', expected '{}'",
            mapping_path.display(),
            mapping_row.source_task_id,
            fact_id
        )));
    }
    Ok(mapping.mappings.len())
}

fn is_exact_mapping_page_id(page_id: &str, game: &str) -> bool {
    let Some((page_game, page)) = page_id.split_once('/') else {
        return false;
    };
    !page.contains('/')
        && page_game == game
        && canonical_resource_identifier("mapping page game", page_game)
            .is_ok_and(|value| value == page_game)
        && canonical_resource_identifier("mapping page", page).is_ok_and(|value| value == page)
}

#[derive(Debug, Clone)]
pub struct ResolvedResourceRoot {
    pub input: PathBuf,
    pub root: PathBuf,
    pub layout: &'static str,
}

pub fn resolve_resource_root(input: &Path) -> ResolvedResourceRoot {
    if looks_like_resource_root(input) {
        return ResolvedResourceRoot {
            input: input.to_path_buf(),
            root: input.to_path_buf(),
            layout: "direct",
        };
    }
    let ours = input.join("ours");
    if looks_like_resource_root(&ours) {
        return ResolvedResourceRoot {
            input: input.to_path_buf(),
            root: ours,
            layout: "repo_ours",
        };
    }
    ResolvedResourceRoot {
        input: input.to_path_buf(),
        root: input.to_path_buf(),
        layout: "unresolved",
    }
}

fn looks_like_resource_root(path: &Path) -> bool {
    path.join("operations").is_dir()
        && (path.join("recognition").is_dir() || path.join("navigation").is_dir())
}

#[derive(Debug)]
pub struct OperationConverter {
    pub root: PathBuf,
    pub game: String,
    pub server: String,
    pub locale: String,
    pub coordinate_space: Value,
    pub defaults: Value,
    resource_ids: HashSet<String>,
    pub bundles: Vec<Bundle>,
    existing_navigation: Option<Value>,
    maa_task_overlays: HashMap<String, Value>,
}

fn write_outputs(outputs: &ConvertOutputs, repo: &Path) -> CliOutcome<()> {
    let game = required_string(&outputs.pack, "game")?;
    let server = required_string(&outputs.pack, "server")?;
    let stem = format!("{game}.{server}");
    write_json_file(
        &repo.join("recognition").join(format!("{stem}.pack.json")),
        &outputs.pack,
    )?;
    write_json_file(
        &repo.join("recognition").join(format!("{stem}.pages.json")),
        &outputs.pages,
    )?;
    write_json_file(
        &repo
            .join("navigation")
            .join(format!("{stem}.navigation.json")),
        &outputs.navigation,
    )?;
    write_json_file(
        &repo.join("operations").join("operations.index.json"),
        &outputs.index,
    )?;
    write_json_file(
        &repo.join("operations").join("operations.primitives.json"),
        &outputs.primitives,
    )
}

impl OperationConverter {
    pub fn load(
        root: &Path,
        game_override: Option<&str>,
        server_override: Option<&str>,
        locale_override: Option<&str>,
    ) -> CliOutcome<Self> {
        let root = root.to_path_buf();
        let ops_dir = root.join("operations");
        let resources = read_json_value(&ops_dir.join("resources.json"))?;
        source::validate_resource_declarations(&ops_dir.join("resources.json"), &resources)?;
        let resource_ids = resource_ids(&resources)?;
        let bundles = load_bundles(&ops_dir)?;
        source::declaration_file_requests(&bundles)?;
        let first = bundles.first().ok_or_else(|| {
            CliError::package_invalid(format!(
                "no Operation Bundles found under {}",
                ops_dir.display()
            ))
        })?;
        let game = game_override
            .map(str::to_string)
            .or_else(|| string_field(&first.data, "game"))
            .ok_or_else(|| {
                CliError::package_invalid(
                    "resource metadata requires game in the first operation bundle or an explicit override",
                )
            })
            .and_then(|value| canonical_game(&value))?;
        let server = server_override
            .map(str::to_string)
            .or_else(|| first_server_scope(&first.data))
            .ok_or_else(|| {
                CliError::package_invalid(
                    "resource metadata requires a non-empty server_scope in the first operation bundle or an explicit override",
                )
            })
            .and_then(|value| canonical_server(&value))?;
        let locale = match locale_override
            .map(str::to_string)
            .or_else(|| string_field(&first.data, "locale"))
        {
            Some(value) => canonical_locale(&value)?,
            None => existing_pack_locale(&root, &game, &server)?.ok_or_else(|| {
                CliError::package_invalid(
                    "resource metadata requires locale in the first operation bundle, an existing matching recognition pack, or an explicit override",
                )
            })?,
        };
        let coordinate_space =
            first.data.get("coordinate_space").cloned().ok_or_else(|| {
                CliError::package_invalid("first bundle missing coordinate_space")
            })?;
        let defaults = first
            .data
            .get("defaults")
            .cloned()
            .ok_or_else(|| CliError::package_invalid("first bundle missing defaults"))?;
        let existing_navigation_path = root
            .join("navigation")
            .join(format!("{game}.{server}.navigation.json"));
        let existing_navigation = if existing_navigation_path.exists() {
            let value = read_json_value(&existing_navigation_path)?;
            source::validate_navigation_declarations(&existing_navigation_path, &value)?;
            Some(value)
        } else {
            None
        };
        let converter = Self {
            root,
            game,
            server,
            locale,
            coordinate_space,
            defaults,
            resource_ids,
            bundles,
            existing_navigation,
            maa_task_overlays: HashMap::new(),
        };
        converter
            .core()
            .validate_bundles(&capture_files(&converter))?;
        Ok(converter)
    }

    pub(super) fn load_maa_task_overlays(&mut self, tasks_root: &Path) -> CliOutcome<()> {
        let graph = maa_task_graph::compile_maa_task_graph(tasks_root)?;
        self.maa_task_overlays = graph
            .tasks()
            .iter()
            .map(|(task_id, task)| (task_id.clone(), task.clone()))
            .collect();
        Ok(())
    }

    fn core(&self) -> source::OperationConverter {
        source::OperationConverter {
            root: self.root.clone(),
            game: self.game.clone(),
            server: self.server.clone(),
            locale: self.locale.clone(),
            coordinate_space: self.coordinate_space.clone(),
            defaults: self.defaults.clone(),
            resource_ids: self.resource_ids.clone(),
            bundles: self.bundles.clone(),
            existing_navigation: self.existing_navigation.clone(),
            maa_task_overlays: self.maa_task_overlays.clone(),
        }
    }

    pub fn build_all(&self) -> CliOutcome<ConvertOutputs> {
        self.core().build_all(&capture_files(self))
    }

    pub fn build_selected(&self, task_ids: &[String]) -> CliOutcome<ConvertOutputs> {
        self.core().build_selected(task_ids, &capture_files(self))
    }

    pub(crate) fn canonical_task(&self, task_id: &str) -> CliOutcome<Value> {
        self.core().canonical_task(task_id)
    }

    #[cfg(test)]
    fn build_pack(&self) -> CliOutcome<Value> {
        self.core().build_pack(&capture_files(self))
    }

    #[cfg(test)]
    fn build_pages(&self) -> CliOutcome<Value> {
        self.core().build_pages()
    }

    #[cfg(test)]
    fn build_navigation(&self) -> CliOutcome<Value> {
        self.core().build_navigation()
    }

    #[cfg(test)]
    fn prune_page_rules_for_selected_build(
        &self,
        bundles: Vec<Bundle>,
        dependencies: &[Bundle],
    ) -> CliOutcome<Vec<Bundle>> {
        self.core()
            .prune_page_rules_for_selected_build(bundles, dependencies)
    }
}

fn capture_bundle_files(bundles: &[Bundle]) -> BTreeMap<PathBuf, SourceFile> {
    source::source_file_requests(bundles)
        .into_iter()
        .map(|(path, request)| {
            let metadata = fs::metadata(&path);
            let is_file = metadata.as_ref().is_ok_and(|metadata| metadata.is_file());
            let length = metadata
                .map(|metadata| metadata.len())
                .map_err(|error| error.to_string());
            let bytes = match request {
                SourceRead::Metadata => Err("source bytes were not requested".to_string()),
                SourceRead::Bytes => fs::read(&path).map_err(|error| error.to_string()),
                SourceRead::BoundedBytes(limit) => match &length {
                    Ok(size) if *size <= limit => {
                        let mut bytes = Vec::new();
                        fs::File::open(&path)
                            .and_then(|file| {
                                file.take(size.saturating_add(1)).read_to_end(&mut bytes)
                            })
                            .map(|_| bytes)
                            .map_err(|error| error.to_string())
                    }
                    Ok(_) => Err("ocr_fields_dictionary_limit_exceeded".to_string()),
                    Err(error) => Err(error.clone()),
                },
            };
            (
                path,
                SourceFile {
                    is_file,
                    length,
                    bytes,
                },
            )
        })
        .collect()
}

fn capture_files(converter: &OperationConverter) -> ConversionFiles {
    let path = converter.root.join("navigation").join(format!(
        "{}.{}.projection.json",
        converter.game, converter.server
    ));
    let projection_exists = path.try_exists().map_err(|error| error.to_string());
    let projection_bytes = match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    };
    ConversionFiles {
        files: capture_bundle_files(&converter.bundles).into(),
        projection_exists,
        projection_bytes,
    }
}

#[cfg(test)]
fn validate_post_admission_ocr_bundle(bundle: &Bundle) -> CliOutcome<()> {
    source::validate_post_admission_ocr_bundle(
        bundle,
        &ConversionFiles {
            files: capture_bundle_files(std::slice::from_ref(bundle)).into(),
            projection_bytes: Ok(None),
            projection_exists: Ok(false),
        },
    )
}

fn load_bundles(ops_dir: &Path) -> CliOutcome<Vec<Bundle>> {
    let mut entries = fs::read_dir(ops_dir)
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", ops_dir.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            CliError::package_invalid(format!("failed to read {}: {err}", ops_dir.display()))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut bundles = Vec::new();
    for entry in entries {
        let dir = entry.path();
        let task_json = dir.join("task.json");
        if !dir.is_dir() || !task_json.is_file() {
            continue;
        }
        let data = read_json_value(&task_json)?;
        let task_id = required_string(&data, "task_id")?;
        bundles.push(Bundle { task_id, dir, data });
    }
    Ok(bundles)
}

fn read_json_value(path: &Path) -> CliOutcome<Value> {
    let text = fs::read_to_string(path).map_err(|err| {
        CliError::package_invalid(format!("failed to read {}: {err}", path.display()))
    })?;
    serde_json::from_str(&text).map_err(|err| {
        CliError::package_invalid(format!("failed to parse {}: {err}", path.display()))
    })
}

fn write_json_file(path: &Path, value: &Value) -> CliOutcome<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::package_invalid(format!("failed to create {}: {err}", parent.display()))
        })?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(|err| {
        CliError::package_invalid(format!("failed to serialize {}: {err}", path.display()))
    })?;
    text.push('\n');
    fs::write(path, text).map_err(|err| {
        CliError::package_invalid(format!("failed to write {}: {err}", path.display()))
    })
}

fn existing_pack_locale(root: &Path, game: &str, server: &str) -> CliOutcome<Option<String>> {
    let path = root
        .join("recognition")
        .join(format!("{game}.{server}.pack.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let pack = read_json_value(&path)?;
    let pack_game = canonical_game(&required_string(&pack, "game")?)?;
    let pack_server = canonical_server(&required_string(&pack, "server")?)?;
    if pack_game != game || pack_server != server {
        return Err(CliError::package_invalid(format!(
            "recognition pack {} declares {pack_game}.{pack_server}, expected {game}.{server}",
            path.display()
        )));
    }
    canonical_locale(&required_string(&pack, "locale")?).map(Some)
}

#[cfg(test)]
mod tests;
