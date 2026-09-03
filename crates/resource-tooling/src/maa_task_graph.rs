// SPDX-License-Identifier: AGPL-3.0-only

//! MAA task graph expansion at the resource-data boundary.
//!
//! This module consumes MAA task JSON data and implements the public task-schema
//! semantics needed before ActingCommand can convert those resources into its own
//! schema. It does not call or copy the upstream MAA engine.

use crate::JsonDocument;
use actingcommand_contract::{LabError as CliError, LabResult as CliOutcome};
use serde::Serialize;
#[cfg(test)]
use serde_json::json;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const LIST_FIELDS: [&str; 5] = [
    "sub",
    "next",
    "onErrorNext",
    "exceededNext",
    "reduceOtherTimes",
];

// Exact cycle detection cannot catch @-composition names that grow every step.
const MAX_MAA_EXPANSION_DEPTH: usize = 64;
const MAX_MAA_DIRECTORY_DEPTH: usize = 32;
const MAX_MAA_JSON_FILES: usize = 16_384;
const MAX_MAA_JSON_FILE_BYTES: u64 = 67_108_864;
const MAX_MAA_AGGREGATE_JSON_BYTES: u64 = 1_073_741_824;
const MAX_MAA_RAW_TASKS: usize = 65_536;

const CORE_STRING_FIELDS: [&str; 3] = ["Doc", "algorithm", "action"];
const GEOMETRY_FIELDS: [&str; 3] = ["roi", "rectMove", "specificRect"];

const ALGORITHM_SPECIFIC_FIELDS: [&str; 18] = [
    "template", // AsstTypes.h MatchTaskInfo::templ_names / FeatureMatchTaskInfo::templ_names; TaskData.cpp:830 consumes the same key.
    "templThreshold", // AsstTypes.h MatchTaskInfo::templ_thresholds.
    "maskRange", // AsstTypes.h MatchTaskInfo::mask_ranges.
    "colorScales", // AsstTypes.h MatchTaskInfo::color_scales.
    "colorWithClose", // AsstTypes.h MatchTaskInfo::color_close.
    "pureColor", // AsstTypes.h MatchTaskInfo::pure_color.
    "method",   // AsstTypes.h MatchTaskInfo::methods.
    "text",     // AsstTypes.h OcrTaskInfo::text.
    "ocrReplace", // AsstTypes.h OcrTaskInfo::replace_map.
    "fullMatch", // AsstTypes.h OcrTaskInfo::full_match.
    "replaceFull", // AsstTypes.h OcrTaskInfo::replace_full.
    "isAscii",  // AsstTypes.h OcrTaskInfo::is_ascii.
    "withoutDet", // AsstTypes.h OcrTaskInfo::without_det.
    "useRaw",   // AsstTypes.h OcrTaskInfo::use_raw.
    "binThreshold", // AsstTypes.h OcrTaskInfo::bin_threshold.
    "count",    // AsstTypes.h FeatureMatchTaskInfo::count.
    "ratio",    // AsstTypes.h FeatureMatchTaskInfo::ratio.
    "detector", // AsstTypes.h FeatureMatchTaskInfo::detector.
];

const RECOGNITION_FIELDS: [&str; 18] = [
    "threshold",
    "templThreshold",
    "maskRange",
    "colorScales",
    "colorWithClose",
    "pureColor",
    "method",
    "text",
    "ocrReplace",
    "fullMatch",
    "replaceFull",
    "isAscii",
    "withoutDet",
    "useRaw",
    "binThreshold",
    "count",
    "ratio",
    "detector",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaaFactOrigin {
    Declared,
    Inherited,
    Composed,
    MaaDefaulted,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MaaFactSource {
    pub source_task_id: String,
    pub source_json_path: String,
    pub source_file_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MaaFact {
    #[serde(flatten)]
    pub value: MaaFactValue,
    pub source_task_id: String,
    pub source_json_path: String,
    pub source_file_sha256: String,
    pub origin: MaaFactOrigin,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contributing_sources: Vec<MaaFactSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaaFactValue {
    Null,
    Boolean { value: bool },
    Integer { value: i64 },
    Unsigned { value: u64 },
    Number { value: f64 },
    String { value: String },
    Array { items: Vec<MaaFact> },
    Object { fields: BTreeMap<String, MaaFact> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MaaTaskFacts {
    pub task_id: MaaFact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<MaaFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<MaaFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<MaaFact>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub geometry: BTreeMap<String, MaaFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<MaaFact>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub recognition_parameters: BTreeMap<String, MaaFact>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub topology: BTreeMap<String, Vec<MaaFact>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaaFactTrace {
    origin: MaaFactOrigin,
    primary: MaaFactSource,
    contributors: Vec<MaaFactSource>,
}

impl MaaFactTrace {
    fn declared(primary: MaaFactSource) -> Self {
        Self {
            origin: MaaFactOrigin::Declared,
            primary,
            contributors: Vec::new(),
        }
    }

    fn with_origin(mut self, origin: MaaFactOrigin) -> Self {
        self.origin = origin;
        self
    }

    fn with_contributor(mut self, source: MaaFactSource) -> Self {
        if source != self.primary && !self.contributors.contains(&source) {
            self.contributors.push(source);
        }
        self
    }

    fn composed_with(mut self, other: &Self) -> Self {
        self.origin = MaaFactOrigin::Composed;
        self = self.with_contributor(other.primary.clone());
        for source in &other.contributors {
            self = self.with_contributor(source.clone());
        }
        self
    }
}

#[derive(Debug, Clone)]
struct MaaTaskProvenance {
    task: MaaFactTrace,
    fields: BTreeMap<String, MaaFactTrace>,
    list_items: BTreeMap<String, Vec<MaaFactTrace>>,
}

#[derive(Debug, Clone)]
struct TracedMaaTask {
    data: Map<String, Value>,
    provenance: MaaTaskProvenance,
}

#[derive(Debug, Clone)]
pub struct MaaTaskGraph {
    tasks: BTreeMap<String, Value>,
    provenance: BTreeMap<String, MaaTaskProvenance>,
    stats: MaaTaskGraphStats,
}

impl MaaTaskGraph {
    pub(crate) fn task(&self, task_id: &str) -> Option<&Value> {
        self.tasks.get(task_id)
    }

    pub fn stats(&self) -> MaaTaskGraphStats {
        self.stats
    }

    pub fn task_ids(&self) -> Vec<String> {
        self.tasks.keys().cloned().collect()
    }

    pub fn task_document(&self, task_id: &str) -> CliOutcome<JsonDocument> {
        self.task(task_id)
            .cloned()
            .map(JsonDocument::new)
            .ok_or_else(|| {
                CliError::package_invalid(format!("compiled MAA task '{task_id}' was not found"))
            })
    }

    pub fn task_facts(&self, task_id: &str) -> CliOutcome<MaaTaskFacts> {
        let task = self.tasks.get(task_id).ok_or_else(|| {
            CliError::package_invalid(format!("compiled MAA task '{task_id}' was not found"))
        })?;
        let provenance = self.provenance.get(task_id).ok_or_else(|| {
            CliError::package_invalid(format!(
                "compiled MAA task '{task_id}' has no exact fact provenance"
            ))
        })?;
        build_task_facts(task_id, task, provenance)
    }

    pub(crate) fn tasks(&self) -> &BTreeMap<String, Value> {
        &self.tasks
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaaTaskGraphStats {
    pub source_files: usize,
    pub raw_tasks: usize,
    pub compiled_tasks: usize,
    pub base_task_derivations: usize,
    pub explicit_at_tasks: usize,
    pub implicit_at_tasks: usize,
    pub virtual_references: usize,
}

pub fn compile_maa_task_graph(tasks_root: &Path) -> CliOutcome<MaaTaskGraph> {
    compile_maa_task_graph_with_limits(tasks_root, MaaIntakeLimits::PRODUCTION)
}

fn compile_maa_task_graph_with_limits(
    tasks_root: &Path,
    limits: MaaIntakeLimits,
) -> CliOutcome<MaaTaskGraph> {
    let mut files = collect_maa_task_files(tasks_root, limits)?;
    if files.is_empty() {
        return Err(CliError::package_invalid(
            "no MAA task JSON files found under the selected root",
        ));
    }
    files.sort_by(|left, right| left.source_json_path.cmp(&right.source_json_path));
    if let Some(index) = files
        .iter()
        .position(|file| file.path.file_name().and_then(|name| name.to_str()) == Some("tasks.json"))
    {
        let root_tasks = files.remove(index);
        files.insert(0, root_tasks);
    }

    let mut registry = MaaRawTaskRegistry::with_limit(limits.max_raw_tasks);
    for file in &files {
        registry.load_file(file)?;
    }

    MaaTaskCompiler::new(registry, files.len()).compile_all()
}

#[cfg(test)]
fn compile_maa_task_graph_from_value(root: Value) -> CliOutcome<MaaTaskGraph> {
    let mut registry = MaaRawTaskRegistry::default();
    registry.load_value("<memory>", root)?;
    MaaTaskCompiler::new(registry, 1).compile_all()
}

#[derive(Debug)]
struct MaaRawTaskRegistry {
    tasks: BTreeMap<String, RawMaaTask>,
    max_raw_tasks: usize,
}

#[derive(Debug, Clone)]
struct RawMaaTask {
    task_id: String,
    data: Map<String, Value>,
    provenance: MaaTaskProvenance,
}

impl Default for MaaRawTaskRegistry {
    fn default() -> Self {
        Self::with_limit(MAX_MAA_RAW_TASKS)
    }
}

impl MaaRawTaskRegistry {
    fn with_limit(max_raw_tasks: usize) -> Self {
        Self {
            tasks: BTreeMap::new(),
            max_raw_tasks,
        }
    }

    fn load_file(&mut self, file: &MaaTaskFile) -> CliOutcome<()> {
        let handle = fs::File::open(&file.path).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to read MAA task file {}: {err}",
                file.source_json_path
            ))
        })?;
        let read_limit = file
            .byte_len
            .checked_add(1)
            .ok_or_else(|| CliError::package_invalid("MAA task file read limit overflow"))?;
        let mut bytes = Vec::with_capacity(file.byte_len as usize);
        handle
            .take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(|err| {
                CliError::package_invalid(format!(
                    "failed to read MAA task file {}: {err}",
                    file.source_json_path
                ))
            })?;
        if bytes.len() as u64 != file.byte_len {
            return Err(CliError::package_invalid(format!(
                "MAA task file {} changed size during intake",
                file.source_json_path
            )));
        }
        let value = serde_json::from_slice::<Value>(&bytes).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to parse MAA task file {}: {err}",
                file.source_json_path
            ))
        })?;
        self.load_value_with_source(
            &file.source_json_path,
            &format!("{:x}", Sha256::digest(&bytes)),
            value,
        )
    }

    #[cfg(test)]
    fn load_value(&mut self, source: &str, value: Value) -> CliOutcome<()> {
        let bytes = serde_json::to_vec(&value).map_err(|err| {
            CliError::package_invalid(format!("failed to encode MAA task source {source}: {err}"))
        })?;
        self.load_value_with_source(source, &format!("{:x}", Sha256::digest(bytes)), value)
    }

    fn load_value_with_source(
        &mut self,
        source_json_path: &str,
        source_file_sha256: &str,
        value: Value,
    ) -> CliOutcome<()> {
        let object = value.as_object().ok_or_else(|| {
            CliError::package_invalid(format!(
                "MAA task source {source_json_path} must be a JSON object"
            ))
        })?;
        for (task_id, task_value) in object {
            let data = task_value.as_object().cloned().ok_or_else(|| {
                CliError::package_invalid(format!(
                    "MAA task '{task_id}' in {source_json_path} must be a JSON object"
                ))
            })?;
            validate_core_typed_fields(task_id, &data)?;
            let source = MaaFactSource {
                source_task_id: task_id.clone(),
                source_json_path: source_json_path.to_string(),
                source_file_sha256: source_file_sha256.to_string(),
            };
            let declared = MaaFactTrace::declared(source);
            let fields = data
                .keys()
                .map(|field| (field.clone(), declared.clone()))
                .collect();
            let list_items = LIST_FIELDS
                .into_iter()
                .filter_map(|field| {
                    data.get(field)
                        .and_then(task_list_expressions)
                        .map(|items| {
                            (
                                field.to_string(),
                                items.into_iter().map(|_| declared.clone()).collect(),
                            )
                        })
                })
                .collect();
            self.insert_task(RawMaaTask {
                task_id: task_id.clone(),
                data,
                provenance: MaaTaskProvenance {
                    task: declared,
                    fields,
                    list_items,
                },
            })?;
        }
        Ok(())
    }

    fn insert_task(&mut self, task: RawMaaTask) -> CliOutcome<()> {
        let Some(existing) = self.tasks.get(&task.task_id) else {
            if self.tasks.len() >= self.max_raw_tasks {
                return Err(CliError::package_invalid(format!(
                    "MAA raw task count exceeds {}",
                    self.max_raw_tasks
                )));
            }
            self.tasks.insert(task.task_id.clone(), task);
            return Ok(());
        };
        if task.data.contains_key("baseTask") {
            self.tasks.insert(task.task_id.clone(), task);
            return Ok(());
        }
        let mut inherited = existing.data.clone();
        merge_object(&mut inherited, &task.data);
        let mut fields = existing.provenance.fields.clone();
        fields.extend(task.provenance.fields.clone());
        let mut list_items = existing.provenance.list_items.clone();
        for field in task.data.keys() {
            list_items.remove(field);
        }
        list_items.extend(task.provenance.list_items.clone());
        let mut task_trace = task
            .provenance
            .task
            .clone()
            .with_contributor(existing.provenance.task.primary.clone());
        for source in &existing.provenance.task.contributors {
            task_trace = task_trace.with_contributor(source.clone());
        }
        self.tasks.insert(
            task.task_id.clone(),
            RawMaaTask {
                task_id: task.task_id,
                data: inherited,
                provenance: MaaTaskProvenance {
                    task: task_trace,
                    fields,
                    list_items,
                },
            },
        );
        Ok(())
    }
}

struct MaaTaskCompiler {
    raw: BTreeMap<String, RawMaaTask>,
    materialized: HashMap<String, TracedMaaTask>,
    expanded: HashMap<String, TracedMaaTask>,
    stats: MaaTaskGraphStats,
}

impl MaaTaskCompiler {
    fn new(registry: MaaRawTaskRegistry, source_files: usize) -> Self {
        let raw_tasks = registry.tasks.len();
        Self {
            raw: registry.tasks,
            materialized: HashMap::new(),
            expanded: HashMap::new(),
            stats: MaaTaskGraphStats {
                source_files,
                raw_tasks,
                ..MaaTaskGraphStats::default()
            },
        }
    }

    fn compile_all(mut self) -> CliOutcome<MaaTaskGraph> {
        let task_ids = self.raw.keys().cloned().collect::<Vec<_>>();
        for task_id in task_ids {
            self.expand_task(&task_id, &mut Vec::new())?;
        }
        let referenced = self
            .expanded
            .values()
            .flat_map(|task| task_references(&task.data))
            .filter(|task_id| task_id != "Stop" && !self.expanded.contains_key(task_id))
            .collect::<BTreeSet<_>>();
        for task_id in referenced {
            self.expand_task(&task_id, &mut Vec::new())?;
        }
        let mut tasks = BTreeMap::new();
        let mut provenance = BTreeMap::new();
        for (task_id, task) in self.expanded {
            tasks.insert(task_id.clone(), Value::Object(task.data));
            provenance.insert(task_id, task.provenance);
        }
        self.stats.compiled_tasks = tasks.len();
        Ok(MaaTaskGraph {
            tasks,
            provenance,
            stats: self.stats,
        })
    }

    fn expand_task(&mut self, task_id: &str, stack: &mut Vec<String>) -> CliOutcome<TracedMaaTask> {
        if task_id == "Stop" {
            let trace = MaaFactTrace::declared(MaaFactSource {
                source_task_id: "Stop".to_string(),
                source_json_path: "synthetic/Stop.json".to_string(),
                source_file_sha256: "0".repeat(64),
            })
            .with_origin(MaaFactOrigin::Composed);
            let mut task = empty_traced_task(trace.clone());
            task.data
                .insert("task_id".to_string(), Value::String("Stop".to_string()));
            task.data
                .insert("algorithm".to_string(), Value::String("Stop".to_string()));
            task.provenance
                .fields
                .insert("algorithm".to_string(), trace);
            return Ok(task);
        }
        if let Some(task) = self.expanded.get(task_id) {
            return Ok(task.clone());
        }
        if stack.len() >= MAX_MAA_EXPANSION_DEPTH {
            let chain = expansion_chain_tail(stack, task_id);
            return Err(CliError::package_invalid(format!(
                "MAA expansion depth exceeded, possible @-composition cycle: {chain}"
            )));
        }
        if stack.iter().any(|item| item == task_id) {
            let mut chain = stack.clone();
            chain.push(task_id.to_string());
            return Err(CliError::package_invalid(format!(
                "MAA virtual task cycle detected: {}",
                chain.join(" -> ")
            )));
        }
        let mut task = self.materialize_task(task_id, &mut Vec::new())?;
        stack.push(task_id.to_string());
        for field in LIST_FIELDS {
            let Some(value) = task.data.get(field).cloned() else {
                continue;
            };
            let expressions = task_list_expressions(&value).ok_or_else(|| {
                CliError::package_invalid(format!(
                    "MAA task '{task_id}' field '{field}' must be a string or string array"
                ))
            })?;
            let traces = task
                .provenance
                .list_items
                .get(field)
                .cloned()
                .ok_or_else(|| missing_fact_provenance(task_id, field))?;
            if traces.len() != expressions.len() {
                return Err(missing_fact_provenance(task_id, field));
            }
            let inputs = expressions.into_iter().zip(traces).collect::<Vec<_>>();
            let expanded = self.expand_expression_list(task_id, field, &inputs, stack)?;
            task.data.insert(
                field.to_string(),
                Value::Array(
                    expanded
                        .iter()
                        .map(|item| Value::String(item.task_id.clone()))
                        .collect(),
                ),
            );
            task.provenance.list_items.insert(
                field.to_string(),
                expanded.into_iter().map(|item| item.trace).collect(),
            );
        }
        stack.pop();
        self.validate_task_references(task_id, &task.data)?;
        self.expanded.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    fn materialize_task(
        &mut self,
        task_id: &str,
        stack: &mut Vec<String>,
    ) -> CliOutcome<TracedMaaTask> {
        validate_at_component_limit(task_id)?;
        if let Some(task) = self.materialized.get(task_id) {
            return Ok(task.clone());
        }
        if stack.len() >= MAX_MAA_EXPANSION_DEPTH {
            let chain = expansion_chain_tail(stack, task_id);
            return Err(CliError::package_invalid(format!(
                "MAA materialization depth exceeded, possible @-composition chain: {chain}"
            )));
        }
        if stack.iter().any(|item| item == task_id) {
            stack.push(task_id.to_string());
            return Err(CliError::package_invalid(format!(
                "MAA baseTask cycle detected: {}",
                stack.join(" -> ")
            )));
        }
        let split = self.split_materializable_at_task(task_id);
        let raw_task = self.raw.get(task_id).cloned();
        let is_explicit_at = raw_task.is_some() && split.is_some();
        if raw_task.is_none() && split.is_none() {
            return Err(CliError::package_invalid(format!(
                "MAA task '{task_id}' is not defined and cannot be derived as an @ task"
            )));
        }

        stack.push(task_id.to_string());
        let mut task = match raw_task {
            Some(raw) => {
                let base_task = raw.data.get("baseTask").and_then(Value::as_str);
                let mut base = match base_task {
                    Some("#none") => empty_traced_task(raw.provenance.task.clone()),
                    Some(base_id) => {
                        self.stats.base_task_derivations += 1;
                        mark_task_origin(
                            self.materialize_task(base_id, stack)?,
                            MaaFactOrigin::Inherited,
                            Some(raw.provenance.task.clone()),
                        )
                    }
                    None => match split {
                        Some((prefix, base_id)) => {
                            self.stats.explicit_at_tasks += 1;
                            let base = mark_task_origin(
                                self.materialize_task(base_id, stack)?,
                                MaaFactOrigin::Composed,
                                Some(raw.provenance.task.clone()),
                            );
                            rebase_task_list_defaults(base, prefix)
                        }
                        None => empty_traced_task(raw.provenance.task.clone()),
                    },
                };
                filter_algorithm_specific_inheritance(&mut base.data, &raw.data);
                retain_existing_field_provenance(&mut base);
                merge_traced_task(&mut base, &raw);
                base.data.remove("baseTask");
                base.provenance.fields.remove("baseTask");
                base.provenance.list_items.remove("baseTask");
                // MAA task-schema.md lines 217 and 232-233: a derived
                // template-matching task defaults template to its own task name.
                let should_default_template = (is_explicit_at || base_task.is_some())
                    && !raw.data.contains_key("template")
                    && looks_like_template_task(&base.data);
                if should_default_template {
                    base.data.insert(
                        "template".to_string(),
                        Value::String(default_template_name(task_id)),
                    );
                    base.provenance.fields.insert(
                        "template".to_string(),
                        raw.provenance
                            .task
                            .clone()
                            .with_origin(MaaFactOrigin::MaaDefaulted),
                    );
                }
                base.provenance.task = raw.provenance.task;
                base
            }
            None => {
                let (prefix, base_id) = split.expect("checked split");
                self.stats.implicit_at_tasks += 1;
                let base = mark_task_origin(
                    self.materialize_task(base_id, stack)?,
                    MaaFactOrigin::Composed,
                    None,
                );
                rebase_task_list_defaults(base, prefix)
            }
        };
        stack.pop();
        task.data
            .insert("task_id".to_string(), Value::String(task_id.to_string()));
        self.materialized.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    fn expand_expression_list(
        &mut self,
        task_id: &str,
        field: &str,
        expressions: &[(String, MaaFactTrace)],
        stack: &mut Vec<String>,
    ) -> CliOutcome<Vec<ResolvedTaskRef>> {
        let mut out = Vec::new();
        for (expression, trace) in expressions {
            let mut parser =
                MaaExpressionParser::new(self, task_id, field, expression, trace.clone(), stack);
            merge_unique_refs(&mut out, parser.parse()?);
        }
        Ok(out)
    }

    fn validate_task_references(
        &mut self,
        owner: &str,
        task: &Map<String, Value>,
    ) -> CliOutcome<()> {
        let mut errors = Vec::new();
        for field in LIST_FIELDS {
            for target in
                task_list_expressions(task.get(field).unwrap_or(&Value::Null)).unwrap_or_default()
            {
                if target == "Stop" {
                    continue;
                }
                if let Err(err) = self.materialize_task(&target, &mut Vec::new()) {
                    errors.push(format!(
                        "{field} references unresolved task '{target}': {}",
                        err.message
                    ));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CliError::package_invalid(format!(
                "MAA task '{owner}' has unresolved references:\n  - {}",
                errors.join("\n  - ")
            )))
        }
    }

    fn split_materializable_at_task<'a>(&self, task_id: &'a str) -> Option<(&'a str, &'a str)> {
        if at_component_count(task_id) > MAX_MAA_EXPANSION_DEPTH {
            return None;
        }
        for (index, _) in task_id.match_indices('@') {
            let prefix = &task_id[..index];
            let base = &task_id[index + 1..];
            if prefix.is_empty() || base.is_empty() {
                continue;
            }
            if self.can_be_template_base(base, &mut HashSet::new()) {
                return Some((prefix, base));
            }
        }
        None
    }

    fn can_be_template_base(&self, task_id: &str, seen: &mut HashSet<String>) -> bool {
        if at_component_count(task_id) > MAX_MAA_EXPANSION_DEPTH {
            return false;
        }
        if self.raw.contains_key(task_id) {
            return true;
        }
        if !seen.insert(task_id.to_string()) {
            return false;
        }
        task_id.match_indices('@').any(|(index, _)| {
            let prefix = &task_id[..index];
            let base = &task_id[index + 1..];
            !prefix.is_empty() && !base.is_empty() && self.can_be_template_base(base, seen)
        })
    }

    fn expand_virtual_field(
        &mut self,
        context_task: &str,
        left: &[ResolvedTaskRef],
        sharp_type: &str,
        expression_trace: &MaaFactTrace,
        stack: &mut Vec<String>,
    ) -> CliOutcome<Vec<ResolvedTaskRef>> {
        self.stats.virtual_references += 1;
        match sharp_type {
            "none" => Ok(Vec::new()),
            "self" => Ok(vec![ResolvedTaskRef {
                task_id: context_task.to_string(),
                trace: expression_trace
                    .clone()
                    .with_origin(MaaFactOrigin::Composed),
            }]),
            // MAA task-schema.md lines 245-248: bare #back is skipped;
            // non-bare X#back returns X.
            "back" => Ok(left
                .iter()
                .cloned()
                .map(|mut item| {
                    item.trace.origin = MaaFactOrigin::Composed;
                    item
                })
                .collect()),
            "next" | "sub" | "on_error_next" | "exceeded_next" | "reduce_other_times" => {
                let field = sharp_field_name(sharp_type);
                let mut out = Vec::new();
                for left_item in left {
                    let task = self.expand_task(&left_item.task_id, stack)?;
                    let value = task.data.get(field).cloned().unwrap_or(Value::Null);
                    let values = task_list_expressions(&value).unwrap_or_default();
                    let traces = task
                        .provenance
                        .list_items
                        .get(field)
                        .cloned()
                        .unwrap_or_default();
                    if values.len() != traces.len() {
                        return Err(missing_fact_provenance(&left_item.task_id, field));
                    }
                    let resolved = values
                        .into_iter()
                        .zip(traces)
                        .map(|(task_id, trace)| ResolvedTaskRef {
                            task_id,
                            trace: left_item.trace.clone().composed_with(&trace),
                        })
                        .collect();
                    merge_unique_refs(&mut out, resolved);
                }
                Ok(out)
            }
            other => Err(CliError::package_invalid(format!(
                "unknown MAA virtual task '#{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedTaskRef {
    task_id: String,
    trace: MaaFactTrace,
}

struct MaaExpressionParser<'a> {
    compiler: &'a mut MaaTaskCompiler,
    stack: &'a mut Vec<String>,
    context_task: &'a str,
    field: &'a str,
    input: &'a str,
    expression_trace: MaaFactTrace,
    chars: Vec<char>,
    pos: usize,
}

impl<'a> MaaExpressionParser<'a> {
    fn new(
        compiler: &'a mut MaaTaskCompiler,
        context_task: &'a str,
        field: &'a str,
        input: &'a str,
        expression_trace: MaaFactTrace,
        stack: &'a mut Vec<String>,
    ) -> Self {
        Self {
            compiler,
            stack,
            context_task,
            field,
            input,
            expression_trace,
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse(&mut self) -> CliOutcome<Vec<ResolvedTaskRef>> {
        let result = self.parse_union_diff()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(self.error("unexpected trailing input"));
        }
        Ok(result)
    }

    fn parse_union_diff(&mut self) -> CliOutcome<Vec<ResolvedTaskRef>> {
        let mut left = self.parse_repeat()?;
        loop {
            self.skip_ws();
            if self.consume('+') {
                mark_refs_composed(&mut left);
                let mut right = self.parse_repeat()?;
                mark_refs_composed(&mut right);
                merge_unique_refs(&mut left, right);
            } else if self.consume('^') {
                mark_refs_composed(&mut left);
                let right = self.parse_repeat()?;
                let banned = right
                    .into_iter()
                    .map(|item| item.task_id)
                    .collect::<HashSet<_>>();
                left.retain(|item| !banned.contains(&item.task_id));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_repeat(&mut self) -> CliOutcome<Vec<ResolvedTaskRef>> {
        let mut value = self.parse_at_sharp()?;
        self.skip_ws();
        if self.consume('*') {
            self.parse_usize()?;
            // Repetition cannot change the normalized de-duplicated task list.
            // Mark the surviving values as composed without allocating repeats.
            mark_refs_composed(&mut value);
        }
        Ok(value)
    }

    fn parse_at_sharp(&mut self) -> CliOutcome<Vec<ResolvedTaskRef>> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.consume('@') {
                let right = self.parse_unary()?;
                left = combine_at_task_refs(&left, &right);
            } else if self.consume('#') {
                let sharp_type = self.parse_ident()?;
                left = self.compiler.expand_virtual_field(
                    self.context_task,
                    &left,
                    &sharp_type,
                    &self.expression_trace,
                    self.stack,
                )?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> CliOutcome<Vec<ResolvedTaskRef>> {
        self.skip_ws();
        if self.consume('(') {
            let value = self.parse_union_diff()?;
            self.skip_ws();
            if !self.consume(')') {
                return Err(self.error("missing ')'"));
            }
            return Ok(value);
        }
        if self.consume('#') {
            let sharp_type = self.parse_ident()?;
            return self.compiler.expand_virtual_field(
                self.context_task,
                &[],
                &sharp_type,
                &self.expression_trace,
                self.stack,
            );
        }
        let ident = self.parse_ident()?;
        if ident.is_empty() {
            return Err(self.error("expected task id"));
        }
        Ok(vec![ResolvedTaskRef {
            task_id: ident,
            trace: self.expression_trace.clone(),
        }])
    }

    fn parse_ident(&mut self) -> CliOutcome<String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.chars.len() {
            let current = self.chars[self.pos];
            if matches!(
                current,
                '#' | '+' | '^' | '*' | '(' | ')' | ' ' | '\t' | '\r' | '\n'
            ) {
                break;
            }
            if current == '@'
                && self
                    .chars
                    .get(self.pos + 1)
                    .is_some_and(|next| matches!(next, '(' | '#'))
            {
                break;
            }
            self.pos += 1;
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn parse_usize(&mut self) -> CliOutcome<usize> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.error("expected repeat count"));
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse::<usize>()
            .map_err(|err| self.error(format!("invalid repeat count: {err}")))
    }

    fn consume(&mut self, expected: char) -> bool {
        self.skip_ws();
        if self.pos < self.chars.len() && self.chars[self.pos] == expected {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn error(&self, message: impl Into<String>) -> CliError {
        CliError::package_invalid(format!(
            "failed to parse MAA task expression '{}' in {}.{}: {}",
            self.input,
            self.context_task,
            self.field,
            message.into()
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct MaaIntakeLimits {
    max_directory_depth: usize,
    max_json_files: usize,
    max_json_file_bytes: u64,
    max_aggregate_json_bytes: u64,
    max_raw_tasks: usize,
}

impl MaaIntakeLimits {
    const PRODUCTION: Self = Self {
        max_directory_depth: MAX_MAA_DIRECTORY_DEPTH,
        max_json_files: MAX_MAA_JSON_FILES,
        max_json_file_bytes: MAX_MAA_JSON_FILE_BYTES,
        max_aggregate_json_bytes: MAX_MAA_AGGREGATE_JSON_BYTES,
        max_raw_tasks: MAX_MAA_RAW_TASKS,
    };
}

#[derive(Debug, Clone)]
struct MaaTaskFile {
    path: PathBuf,
    source_json_path: String,
    byte_len: u64,
}

fn collect_maa_task_files(root: &Path, limits: MaaIntakeLimits) -> CliOutcome<Vec<MaaTaskFile>> {
    let root_metadata = fs::symlink_metadata(root).map_err(|err| {
        CliError::package_invalid(format!("failed to inspect MAA task root: {err}"))
    })?;
    if root_metadata.file_type().is_symlink() {
        return Err(CliError::package_invalid(
            "MAA task root must not be a symbolic link",
        ));
    }
    if !root_metadata.is_dir() {
        return Err(CliError::package_invalid(
            "MAA task root must be a directory",
        ));
    }
    let mut files = Vec::new();
    let mut aggregate_bytes = 0u64;
    collect_maa_task_files_inner(root, root, 0, limits, &mut aggregate_bytes, &mut files)?;
    Ok(files)
}

fn collect_maa_task_files_inner(
    task_root: &Path,
    directory: &Path,
    depth: usize,
    limits: MaaIntakeLimits,
    aggregate_bytes: &mut u64,
    files: &mut Vec<MaaTaskFile>,
) -> CliOutcome<()> {
    let entries = fs::read_dir(directory).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read MAA task directory at depth {depth}: {err}"
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::package_invalid(format!(
                "failed to read MAA task directory entry at depth {depth}: {err}"
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| {
            CliError::package_invalid(format!(
                "failed to inspect MAA task directory entry at depth {depth}: {err}"
            ))
        })?;
        if file_type.is_symlink() {
            let relative = normalized_source_json_path(task_root, &path)?;
            return Err(CliError::package_invalid(format!(
                "MAA task intake encountered symbolic link: {relative}"
            )));
        }
        if file_type.is_dir() {
            let next_depth = depth
                .checked_add(1)
                .ok_or_else(|| CliError::package_invalid("MAA task directory depth overflow"))?;
            if next_depth > limits.max_directory_depth {
                return Err(CliError::package_invalid(format!(
                    "MAA task directory depth exceeds {}",
                    limits.max_directory_depth
                )));
            }
            collect_maa_task_files_inner(
                task_root,
                &path,
                next_depth,
                limits,
                aggregate_bytes,
                files,
            )?;
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("json")
        {
            if files.len() >= limits.max_json_files {
                return Err(CliError::package_invalid(format!(
                    "MAA JSON file count exceeds {}",
                    limits.max_json_files
                )));
            }
            let source_json_path = normalized_source_json_path(task_root, &path)?;
            let byte_len = entry
                .metadata()
                .map_err(|err| {
                    CliError::package_invalid(format!(
                        "failed to inspect MAA task file {source_json_path}: {err}"
                    ))
                })?
                .len();
            if byte_len > limits.max_json_file_bytes {
                return Err(CliError::package_invalid(format!(
                    "MAA JSON file bytes exceed {}: {source_json_path}",
                    limits.max_json_file_bytes
                )));
            }
            *aggregate_bytes = aggregate_bytes.checked_add(byte_len).ok_or_else(|| {
                CliError::package_invalid("MAA aggregate JSON byte count overflow")
            })?;
            if *aggregate_bytes > limits.max_aggregate_json_bytes {
                return Err(CliError::package_invalid(format!(
                    "MAA aggregate JSON bytes exceed {}",
                    limits.max_aggregate_json_bytes
                )));
            }
            files.push(MaaTaskFile {
                path,
                source_json_path,
                byte_len,
            });
        }
    }
    Ok(())
}

fn normalized_source_json_path(root: &Path, path: &Path) -> CliOutcome<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CliError::package_invalid("MAA source path is outside the selected task root")
    })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(CliError::package_invalid(
                "MAA source path cannot be represented as a normalized relative path",
            ));
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| CliError::package_invalid("MAA source path is not valid UTF-8"))?;
        if segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.contains('/')
            || segment.contains('\\')
        {
            return Err(CliError::package_invalid(
                "MAA source path contains an invalid segment",
            ));
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err(CliError::package_invalid("MAA source JSON path is empty"));
    }
    Ok(segments.join("/"))
}

fn merge_object(base: &mut Map<String, Value>, child: &Map<String, Value>) {
    for (key, value) in child {
        base.insert(key.clone(), value.clone());
    }
}

fn expansion_chain_tail(stack: &[String], next_task: &str) -> String {
    let start = stack.len().saturating_sub(8);
    let mut chain = stack[start..].to_vec();
    chain.push(next_task.to_string());
    chain.join(" -> ")
}

fn validate_at_component_limit(task_id: &str) -> CliOutcome<()> {
    let components = at_component_count(task_id);
    if components <= MAX_MAA_EXPANSION_DEPTH {
        return Ok(());
    }
    Err(CliError::package_invalid(format!(
        "MAA task name @-composition components exceed 64: components={components}, task='{}'",
        truncated_task_name(task_id)
    )))
}

fn at_component_count(task_id: &str) -> usize {
    task_id.matches('@').count() + 1
}

fn truncated_task_name(task_id: &str) -> String {
    const LIMIT: usize = 160;
    if task_id.chars().count() <= LIMIT {
        return task_id.to_string();
    }
    let prefix: String = task_id.chars().take(LIMIT).collect();
    format!("{prefix}...")
}

fn rebase_task_list_defaults(mut base: TracedMaaTask, prefix: &str) -> TracedMaaTask {
    // MAA task-schema.md lines 221-234: @ tasks rebase list-field defaults
    // by prefixing task references; non-list defaults follow separate rules.
    for field in LIST_FIELDS {
        let Some(value) = base.data.get(field).cloned() else {
            continue;
        };
        let Some(expressions) = task_list_expressions(&value) else {
            continue;
        };
        base.data.insert(
            field.to_string(),
            Value::Array(
                expressions
                    .into_iter()
                    .map(|expression| Value::String(rebase_expression(prefix, &expression)))
                    .collect(),
            ),
        );
        if let Some(traces) = base.provenance.list_items.get_mut(field) {
            for trace in traces {
                trace.origin = MaaFactOrigin::Composed;
            }
        }
    }
    base
}

fn rebase_expression(prefix: &str, expression: &str) -> String {
    if expression.trim_start().starts_with('#') {
        format!("{prefix}{}", expression.trim_start())
    } else {
        format!("{prefix}@{expression}")
    }
}

fn filter_algorithm_specific_inheritance(
    inherited: &mut Map<String, Value>,
    child: &Map<String, Value>,
) {
    let Some(child_algorithm) = child.get("algorithm").and_then(Value::as_str) else {
        return;
    };
    let Some(parent_algorithm) = inherited.get("algorithm").and_then(Value::as_str) else {
        return;
    };
    if parent_algorithm == child_algorithm {
        return;
    }
    // MAA task-schema.md lines 217-218 and 232-234:
    // when the algorithm changes, only TaskInfo parameters inherit.
    for key in ALGORITHM_SPECIFIC_FIELDS {
        inherited.remove(key);
    }
}

fn looks_like_template_task(task: &Map<String, Value>) -> bool {
    matches!(
        task.get("algorithm").and_then(Value::as_str),
        None | Some("MatchTemplate")
    ) || task.contains_key("template")
}

fn default_template_name(task_id: &str) -> String {
    // MAA task-schema.md lines 217 and 232-233.
    format!("{task_id}.png")
}

fn task_list_expressions(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Null => Some(Vec::new()),
        Value::String(value) => Some(vec![value.to_string()]),
        Value::Array(values) => values
            .iter()
            .map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

fn task_references(task: &Map<String, Value>) -> Vec<String> {
    LIST_FIELDS
        .into_iter()
        .flat_map(|field| {
            task_list_expressions(task.get(field).unwrap_or(&Value::Null)).unwrap_or_default()
        })
        .collect()
}

fn sharp_field_name(sharp_type: &str) -> &str {
    match sharp_type {
        "next" => "next",
        "sub" => "sub",
        "on_error_next" => "onErrorNext",
        "exceeded_next" => "exceededNext",
        "reduce_other_times" => "reduceOtherTimes",
        _ => sharp_type,
    }
}

fn merge_unique_refs(out: &mut Vec<ResolvedTaskRef>, values: Vec<ResolvedTaskRef>) {
    for value in values {
        if let Some(existing) = out.iter_mut().find(|item| item.task_id == value.task_id) {
            existing.trace = existing.trace.clone().composed_with(&value.trace);
        } else {
            out.push(value);
        }
    }
}

fn mark_refs_composed(values: &mut [ResolvedTaskRef]) {
    for value in values {
        value.trace.origin = MaaFactOrigin::Composed;
    }
}

fn combine_at_task_refs(
    left: &[ResolvedTaskRef],
    right: &[ResolvedTaskRef],
) -> Vec<ResolvedTaskRef> {
    let mut out = Vec::new();
    for lhs in left {
        for rhs in right {
            out.push(ResolvedTaskRef {
                task_id: format!("{}@{}", lhs.task_id, rhs.task_id),
                trace: lhs.trace.clone().composed_with(&rhs.trace),
            });
        }
    }
    out
}

fn empty_traced_task(task_trace: MaaFactTrace) -> TracedMaaTask {
    TracedMaaTask {
        data: Map::new(),
        provenance: MaaTaskProvenance {
            task: task_trace,
            fields: BTreeMap::new(),
            list_items: BTreeMap::new(),
        },
    }
}

fn mark_task_origin(
    mut task: TracedMaaTask,
    origin: MaaFactOrigin,
    contributor: Option<MaaFactTrace>,
) -> TracedMaaTask {
    task.provenance.task.origin = origin;
    if let Some(contributor) = contributor.as_ref() {
        task.provenance.task = task
            .provenance
            .task
            .clone()
            .with_contributor(contributor.primary.clone());
        for source in &contributor.contributors {
            task.provenance.task = task
                .provenance
                .task
                .clone()
                .with_contributor(source.clone());
        }
    }
    for trace in task.provenance.fields.values_mut() {
        trace.origin = origin;
        if let Some(contributor) = contributor.as_ref() {
            *trace = trace.clone().with_contributor(contributor.primary.clone());
            for source in &contributor.contributors {
                *trace = trace.clone().with_contributor(source.clone());
            }
        }
    }
    for traces in task.provenance.list_items.values_mut() {
        for trace in traces {
            trace.origin = origin;
            if let Some(contributor) = contributor.as_ref() {
                *trace = trace.clone().with_contributor(contributor.primary.clone());
                for source in &contributor.contributors {
                    *trace = trace.clone().with_contributor(source.clone());
                }
            }
        }
    }
    task
}

fn retain_existing_field_provenance(task: &mut TracedMaaTask) {
    task.provenance
        .fields
        .retain(|field, _| task.data.contains_key(field));
    task.provenance
        .list_items
        .retain(|field, _| task.data.contains_key(field));
}

fn merge_traced_task(base: &mut TracedMaaTask, child: &RawMaaTask) {
    for (field, value) in &child.data {
        base.data.insert(field.clone(), value.clone());
        if let Some(trace) = child.provenance.fields.get(field) {
            base.provenance.fields.insert(field.clone(), trace.clone());
        }
        base.provenance.list_items.remove(field);
        if let Some(traces) = child.provenance.list_items.get(field) {
            base.provenance
                .list_items
                .insert(field.clone(), traces.clone());
        }
    }
}

fn validate_core_typed_fields(task_id: &str, task: &Map<String, Value>) -> CliOutcome<()> {
    if let Some(value) = task.get("baseTask")
        && !value.is_string()
    {
        return Err(malformed_core_field(task_id, "baseTask", "a string"));
    }
    for field in CORE_STRING_FIELDS {
        if let Some(value) = task.get(field)
            && !value.is_string()
        {
            return Err(malformed_core_field(task_id, field, "a string"));
        }
    }
    if let Some(value) = task.get("template") {
        let valid = value.is_string()
            || value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string));
        if !valid {
            return Err(malformed_core_field(
                task_id,
                "template",
                "a string or string array",
            ));
        }
    }
    for field in GEOMETRY_FIELDS {
        if let Some(value) = task.get(field) {
            validate_geometry_field(task_id, field, value)?;
        }
    }
    Ok(())
}

fn validate_geometry_field(task_id: &str, field: &str, value: &Value) -> CliOutcome<()> {
    let valid = match value {
        Value::Array(values) => values.len() == 4 && values.iter().all(Value::is_i64),
        Value::Object(object) => {
            let width = object.get("width").or_else(|| object.get("w"));
            let height = object.get("height").or_else(|| object.get("h"));
            object.get("x").is_some_and(Value::is_i64)
                && object.get("y").is_some_and(Value::is_i64)
                && width.is_some_and(Value::is_i64)
                && height.is_some_and(Value::is_i64)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(malformed_core_field(
            task_id,
            field,
            "four integer coordinates",
        ))
    }
}

fn malformed_core_field(task_id: &str, field: &str, expected: &str) -> CliError {
    CliError::package_invalid(format!(
        "MAA task '{task_id}' core field '{field}' must be {expected}"
    ))
}

fn missing_fact_provenance(task_id: &str, field: &str) -> CliError {
    CliError::package_invalid(format!(
        "MAA task '{task_id}' field '{field}' has no exact fact provenance"
    ))
}

fn build_task_facts(
    task_id: &str,
    task: &Value,
    provenance: &MaaTaskProvenance,
) -> CliOutcome<MaaTaskFacts> {
    let object = task.as_object().ok_or_else(|| {
        CliError::package_invalid(format!("compiled MAA task '{task_id}' is not an object"))
    })?;
    let task_id_fact = build_fact(&Value::String(task_id.to_string()), &provenance.task)?;
    let doc = optional_field_fact(task_id, object, provenance, "Doc")?;
    let algorithm = optional_field_fact(task_id, object, provenance, "algorithm")?;
    let action = optional_field_fact(task_id, object, provenance, "action")?;
    let template = optional_field_fact(task_id, object, provenance, "template")?;

    let mut geometry = BTreeMap::new();
    for field in GEOMETRY_FIELDS {
        if let Some(fact) = optional_field_fact(task_id, object, provenance, field)? {
            geometry.insert(field.to_string(), fact);
        }
    }

    let mut recognition_parameters = BTreeMap::new();
    for field in RECOGNITION_FIELDS {
        if let Some(fact) = optional_field_fact(task_id, object, provenance, field)? {
            recognition_parameters.insert(field.to_string(), fact);
        }
    }

    let mut topology = BTreeMap::new();
    for field in LIST_FIELDS {
        let Some(value) = object.get(field) else {
            continue;
        };
        let values = task_list_expressions(value).ok_or_else(|| {
            CliError::package_invalid(format!(
                "MAA task '{task_id}' field '{field}' must be a string or string array"
            ))
        })?;
        if values.is_empty() {
            continue;
        }
        let traces = provenance
            .list_items
            .get(field)
            .ok_or_else(|| missing_fact_provenance(task_id, field))?;
        if values.len() != traces.len() {
            return Err(missing_fact_provenance(task_id, field));
        }
        let facts = values
            .into_iter()
            .zip(traces)
            .map(|(value, trace)| build_fact(&Value::String(value), trace))
            .collect::<CliOutcome<Vec<_>>>()?;
        topology.insert(field.to_string(), facts);
    }

    Ok(MaaTaskFacts {
        task_id: task_id_fact,
        doc,
        algorithm,
        action,
        geometry,
        template,
        recognition_parameters,
        topology,
    })
}

fn optional_field_fact(
    task_id: &str,
    task: &Map<String, Value>,
    provenance: &MaaTaskProvenance,
    field: &str,
) -> CliOutcome<Option<MaaFact>> {
    let Some(value) = task.get(field) else {
        return Ok(None);
    };
    let trace = provenance
        .fields
        .get(field)
        .ok_or_else(|| missing_fact_provenance(task_id, field))?;
    build_fact(value, trace).map(Some)
}

fn build_fact(value: &Value, trace: &MaaFactTrace) -> CliOutcome<MaaFact> {
    validate_fact_trace(trace)?;
    let typed = match value {
        Value::Null => MaaFactValue::Null,
        Value::Bool(value) => MaaFactValue::Boolean { value: *value },
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                MaaFactValue::Integer { value }
            } else if let Some(value) = value.as_u64() {
                MaaFactValue::Unsigned { value }
            } else {
                MaaFactValue::Number {
                    value: value.as_f64().ok_or_else(|| {
                        CliError::package_invalid("MAA numeric fact cannot be represented exactly")
                    })?,
                }
            }
        }
        Value::String(value) => MaaFactValue::String {
            value: value.clone(),
        },
        Value::Array(values) => MaaFactValue::Array {
            items: values
                .iter()
                .map(|value| build_fact(value, trace))
                .collect::<CliOutcome<Vec<_>>>()?,
        },
        Value::Object(values) => MaaFactValue::Object {
            fields: values
                .iter()
                .map(|(field, value)| Ok((field.clone(), build_fact(value, trace)?)))
                .collect::<CliOutcome<BTreeMap<_, _>>>()?,
        },
    };
    Ok(MaaFact {
        value: typed,
        source_task_id: trace.primary.source_task_id.clone(),
        source_json_path: trace.primary.source_json_path.clone(),
        source_file_sha256: trace.primary.source_file_sha256.clone(),
        origin: trace.origin,
        contributing_sources: trace.contributors.clone(),
    })
}

fn validate_fact_trace(trace: &MaaFactTrace) -> CliOutcome<()> {
    validate_fact_source(&trace.primary)?;
    for source in &trace.contributors {
        validate_fact_source(source)?;
    }
    Ok(())
}

fn validate_fact_source(source: &MaaFactSource) -> CliOutcome<()> {
    let path = &source.source_json_path;
    let path_valid = !path.is_empty()
        && !path.contains('\\')
        && !Path::new(path).is_absolute()
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."));
    let hash_valid = source.source_file_sha256.len() == 64
        && source
            .source_file_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if source.source_task_id.is_empty() || !path_valid || !hash_valid {
        return Err(CliError::package_invalid(
            "MAA fact source path, hash, or task identity cannot be represented exactly",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_base_task_with_child_override() {
        let graph = compile_maa_task_graph_from_value(json!({
            "ClickChapter": {
                "algorithm": "OcrDetect",
                "action": "ClickSelf",
                "roi": [142, 43, 250, 150],
                "text": [],
                "next": ["#back", "#self", "Stop"]
            },
            "ClickChapter2": {
                "baseTask": "ClickChapter",
                "text": ["幻灭"]
            }
        }))
        .unwrap();

        let task = graph.task("ClickChapter2").unwrap();
        assert_eq!(
            task.pointer("/algorithm").and_then(Value::as_str),
            Some("OcrDetect")
        );
        assert_eq!(
            task.pointer("/text/0").and_then(Value::as_str),
            Some("幻灭")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["ClickChapter2", "Stop"]));
    }

    #[test]
    fn expands_implicit_at_task_and_virtual_back_references() {
        let graph = compile_maa_task_graph_from_value(json!({
            "A": { "next": ["N1", "#back"] },
            "N1": { "next": [] },
            "B": { "next": ["Other", "B@A"] },
            "Other": { "next": [] }
        }))
        .unwrap();

        let task = graph.task("B@A").unwrap();
        assert_eq!(task.get("next").unwrap(), &json!(["B@N1", "B"]));
    }

    #[test]
    fn expands_virtual_field_references_from_context() {
        let graph = compile_maa_task_graph_from_value(json!({
            "A": { "next": ["N1", "N2"] },
            "N1": { "next": [] },
            "N2": { "next": [] },
            "C": { "next": ["B@A#next"] }
        }))
        .unwrap();

        let task = graph.task("C").unwrap();
        assert_eq!(task.get("next").unwrap(), &json!(["B@N1", "B@N2"]));
    }

    #[test]
    fn expands_all_virtual_list_field_references() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Source": {
                "sub": ["SubA", "SubB"],
                "onErrorNext": ["Recover"],
                "reduceOtherTimes": ["Cooldown"]
            },
            "Driver": {
                "next": ["UseSub@Source#sub", "Fail@Source#on_error_next", "Limit@Source#reduce_other_times"]
            },
            "SubA": {"next": []},
            "SubB": {"next": []},
            "Recover": {"next": []},
            "Cooldown": {"next": []}
        }))
        .unwrap();

        let task = graph.task("Driver").unwrap();
        assert_eq!(
            task.get("next").unwrap(),
            &json!([
                "UseSub@SubA",
                "UseSub@SubB",
                "Fail@Recover",
                "Limit@Cooldown"
            ])
        );
    }

    #[test]
    fn expands_multi_at_task_id_before_binary_virtual_reference() {
        let graph = compile_maa_task_graph_from_value(json!({
            "QuickSwitch@ToHome": {
                "next": ["QuickSwitch@ToHome@Entry", "QuickSwitch@ToHome@Open"]
            },
            "QuickSwitch@ToHome@Entry": { "next": [] },
            "QuickSwitch@ToHome@Open": { "next": [] },
            "Home": { "next": ["Home@QuickSwitch@ToHome#next"] }
        }))
        .unwrap();

        let task = graph.task("Home").unwrap();
        assert_eq!(
            task.get("next").unwrap(),
            &json!([
                "Home@QuickSwitch@ToHome@Entry",
                "Home@QuickSwitch@ToHome@Open"
            ])
        );
    }

    #[test]
    fn expands_parenthesized_difference_before_at_prefix() {
        let graph = compile_maa_task_graph_from_value(json!({
            "ToChapter2": { "next": ["ClickChapterNew", "ClickChapter2", "Stop"] },
            "ClickChapterNew": { "next": [] },
            "ClickChapter2": { "next": [] },
            "ClickChapter1@ClickChapterNew": { "next": [] },
            "ClickChapter1@ClickChapter2": { "next": [] },
            "ToChapter1": { "next": ["ClickChapter1@(ToChapter2#next^Stop)"] }
        }))
        .unwrap();

        let task = graph.task("ToChapter1").unwrap();
        assert_eq!(
            task.get("next").unwrap(),
            &json!([
                "ClickChapter1@ClickChapterNew",
                "ClickChapter1@ClickChapter2"
            ])
        );
    }

    #[test]
    fn explicit_at_task_rebases_base_lists_and_uses_task_template_default() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "template": "Base.png",
                "next": ["N1", "#back"]
            },
            "N1": { "next": [] },
            "P": { "next": [] },
            "P@N1": { "next": [] },
            "P@Base": {}
        }))
        .unwrap();

        let task = graph.task("P@Base").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("P@Base.png")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["P@N1", "P"]));
    }

    #[test]
    fn implicit_at_task_inherits_base_template() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "template": "Base.png",
                "next": ["N1", "#back"]
            },
            "N1": { "next": [] },
            "P": { "next": [] },
            "P@N1": { "next": [] },
            "Driver": { "next": ["P@Base"] }
        }))
        .unwrap();

        let task = graph.task("P@Base").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Base.png")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["P@N1", "P"]));
    }

    #[test]
    fn explicit_at_with_base_task_uses_declared_base_task() {
        let graph = compile_maa_task_graph_from_value(json!({
            "NameBase": {
                "algorithm": "MatchTemplate",
                "template": "NameBase.png",
                "next": ["NameNext"]
            },
            "DeclaredBase": {
                "algorithm": "MatchTemplate",
                "template": "DeclaredBase.png",
                "next": ["DeclaredNext"]
            },
            "NameNext": { "next": [] },
            "DeclaredNext": { "next": [] },
            "Prefix@NameBase": {
                "baseTask": "DeclaredBase"
            }
        }))
        .unwrap();

        let task = graph.task("Prefix@NameBase").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Prefix@NameBase.png")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["DeclaredNext"]));
    }

    #[test]
    fn bare_back_virtual_reference_is_skipped() {
        let graph = compile_maa_task_graph_from_value(json!({
            "A": {
                "next": ["#back", "Stop"]
            }
        }))
        .unwrap();

        let task = graph.task("A").unwrap();
        assert_eq!(task.get("next").unwrap(), &json!(["Stop"]));
    }

    #[test]
    fn algorithm_change_drops_algorithm_specific_inherited_fields() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "template": "Base.png",
                "templThreshold": 0.87,
                "maskRange": [10, 200],
                "method": "RGBCount",
                "colorScales": [[0, 0, 0]],
                "action": "ClickSelf",
                "roi": [1, 2, 3, 4],
                "next": ["Stop"]
            },
            "Child": {
                "baseTask": "Base",
                "algorithm": "OcrDetect",
                "text": ["OK"]
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert_eq!(
            task.pointer("/algorithm").and_then(Value::as_str),
            Some("OcrDetect")
        );
        assert_eq!(task.pointer("/text/0").and_then(Value::as_str), Some("OK"));
        assert!(task.get("template").is_none());
        assert!(task.get("templThreshold").is_none());
        assert!(task.get("maskRange").is_none());
        assert!(task.get("method").is_none());
        assert!(task.get("colorScales").is_none());
        assert_eq!(
            task.pointer("/action").and_then(Value::as_str),
            Some("ClickSelf")
        );
        assert_eq!(task.get("roi").unwrap(), &json!([1, 2, 3, 4]));
        assert_eq!(task.get("next").unwrap(), &json!(["Stop"]));
    }

    #[test]
    fn base_task_uses_child_template_default_even_when_parent_has_template() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "template": "Base.png",
                "next": ["Stop"]
            },
            "Child": {
                "baseTask": "Base",
                "threshold": 0.92
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Child.png")
        );
        assert_eq!(
            task.pointer("/threshold").and_then(Value::as_f64),
            Some(0.92)
        );
    }

    #[test]
    fn base_task_return_example_uses_child_template_default() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Return": {
                "algorithm": "MatchTemplate",
                "action": "ClickSelf",
                "next": ["Stop"]
            },
            "Return2": {
                "baseTask": "Return"
            }
        }))
        .unwrap();

        let task = graph.task("Return2").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Return2.png")
        );
        assert_eq!(
            task.pointer("/action").and_then(Value::as_str),
            Some("ClickSelf")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["Stop"]));
    }

    #[test]
    fn base_task_return_example_uses_child_template_default_with_implicit_algorithm() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Return": {
                "action": "ClickSelf",
                "next": ["Stop"]
            },
            "Return2": {
                "baseTask": "Return"
            }
        }))
        .unwrap();

        let task = graph.task("Return2").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Return2.png")
        );
        assert_eq!(
            task.pointer("/action").and_then(Value::as_str),
            Some("ClickSelf")
        );
        assert_eq!(task.get("next").unwrap(), &json!(["Stop"]));
    }

    #[test]
    fn base_task_chain_without_template_uses_child_default() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "next": ["Stop"]
            },
            "Middle": {
                "baseTask": "Base",
                "threshold": 0.9
            },
            "Child": {
                "baseTask": "Middle",
                "threshold": 0.95
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Child.png")
        );
        assert_eq!(
            task.pointer("/threshold").and_then(Value::as_f64),
            Some(0.95)
        );
    }

    #[test]
    fn base_task_child_template_overrides_parent_template() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "MatchTemplate",
                "template": "Base.png",
                "next": ["Stop"]
            },
            "Child": {
                "baseTask": "Base",
                "template": "ChildExplicit.png"
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("ChildExplicit.png")
        );
    }

    #[test]
    fn multi_file_override_without_base_task_inherits_previous_definition() {
        let mut registry = MaaRawTaskRegistry::default();
        registry
            .load_value(
                "base",
                json!({"A": {"algorithm": "MatchTemplate", "template": "A.png", "next": ["Stop"]}}),
            )
            .unwrap();
        registry
            .load_value("overlay", json!({"A": {"templThreshold": 0.95}}))
            .unwrap();
        let graph = MaaTaskCompiler::new(registry, 2).compile_all().unwrap();

        let task = graph.task("A").unwrap();
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("A.png")
        );
        assert_eq!(
            task.pointer("/templThreshold").and_then(Value::as_f64),
            Some(0.95)
        );
    }

    #[test]
    fn base_task_cycle_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"baseTask": "B"},
            "B": {"baseTask": "A"}
        }))
        .unwrap_err();

        assert!(err.message.contains("baseTask cycle"));
    }

    #[test]
    fn virtual_self_cycle_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["A#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("virtual task cycle"));
        assert!(err.message.contains("A -> A"));
    }

    #[test]
    fn virtual_two_node_cycle_fails_loudly_with_chain() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["B#next"]},
            "B": {"next": ["A#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("virtual task cycle"));
        assert!(err.message.contains("A -> B -> A") || err.message.contains("B -> A -> B"));
    }

    #[test]
    fn virtual_at_composition_growth_cycle_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["A@A#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("expansion depth exceeded"));
        assert!(err.message.contains("possible @-composition cycle"));
    }

    #[test]
    fn virtual_cross_at_composition_growth_cycle_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["B@A#next"]},
            "B": {"next": ["A@B#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("expansion depth exceeded"));
        assert!(err.message.contains("possible @-composition cycle"));
    }

    #[test]
    fn at_task_name_components_5000_fails_without_stack_overflow() {
        let task_name = at_chain_name(5000);
        let err = compile_maa_task_graph_from_value(json!({
            "Base": {"algorithm": "MatchTemplate"},
            "Driver": {"next": [task_name]}
        }))
        .unwrap_err();

        assert!(err.message.contains("@-composition components exceed 64"));
    }

    #[test]
    fn at_task_name_components_63_and_64_are_allowed() {
        for components in [63, 64] {
            let task_name = at_chain_name(components);
            let graph = compile_maa_task_graph_from_value(json!({
                "Base": {"algorithm": "MatchTemplate"},
                "Driver": {"next": [task_name.clone()]}
            }))
            .unwrap();

            assert!(
                graph.task(&task_name).is_some(),
                "components={components} should materialize"
            );
        }
    }

    #[test]
    fn at_task_name_components_65_is_rejected() {
        let task_name = at_chain_name(65);
        let err = compile_maa_task_graph_from_value(json!({
            "Base": {"algorithm": "MatchTemplate"},
            "Driver": {"next": [task_name]}
        }))
        .unwrap_err();

        assert!(err.message.contains("@-composition components exceed 64"));
        assert!(err.message.contains("components=65"));
    }

    #[test]
    fn virtual_three_node_cycle_fails_loudly_with_chain() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["B#next"]},
            "B": {"next": ["C#next"]},
            "C": {"next": ["A#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("virtual task cycle"));
        assert!(
            err.message.contains("A -> B -> C -> A")
                || err.message.contains("B -> C -> A -> B")
                || err.message.contains("C -> A -> B -> C")
        );
    }

    #[test]
    fn nested_expression_virtual_cycle_uses_same_stack() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["(B#next)"]},
            "B": {"next": ["A#next"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("virtual task cycle"));
        assert!(err.message.contains("A -> B -> A") || err.message.contains("B -> A -> B"));
    }

    #[test]
    fn legal_deep_virtual_chain_still_expands() {
        let graph = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["B#next"]},
            "B": {"next": ["C#next"]},
            "C": {"next": ["D"]},
            "D": {"next": ["Stop"]}
        }))
        .unwrap();

        assert_eq!(graph.task("A").unwrap().get("next").unwrap(), &json!(["D"]));
    }

    #[test]
    fn legal_at_composition_chain_below_depth_limit_expands() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {"next": ["N"]},
            "N": {"next": []},
            "P@N": {"next": []},
            "A": {"next": ["P@Base#next"]},
            "P": {"next": []}
        }))
        .unwrap();

        assert_eq!(
            graph.task("A").unwrap().get("next").unwrap(),
            &json!(["P@N"])
        );
    }

    #[test]
    fn algorithm_change_preserves_input_text_task_info_field() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "JustReturn",
                "inputText": "doctor",
                "next": ["Stop"]
            },
            "Child": {
                "baseTask": "Base",
                "algorithm": "MatchTemplate"
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert_eq!(
            task.pointer("/inputText").and_then(Value::as_str),
            Some("doctor")
        );
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Child.png")
        );
    }

    #[test]
    fn algorithm_change_drops_replace_full_ocr_field() {
        let graph = compile_maa_task_graph_from_value(json!({
            "Base": {
                "algorithm": "OcrDetect",
                "replaceFull": true,
                "text": ["Start"],
                "next": ["Stop"]
            },
            "Child": {
                "baseTask": "Base",
                "algorithm": "MatchTemplate"
            }
        }))
        .unwrap();

        let task = graph.task("Child").unwrap();
        assert!(task.get("replaceFull").is_none());
        assert!(task.get("text").is_none());
        assert_eq!(
            task.pointer("/template").and_then(Value::as_str),
            Some("Child.png")
        );
    }

    #[test]
    fn unresolved_reference_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["Missing"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("unresolved references"));
    }

    #[test]
    fn typed_facts_preserve_inherited_composed_and_maa_defaulted_origins() {
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(
            temp.path().join("tasks.json"),
            serde_json::to_vec(&json!({
                "Base": {
                    "algorithm": "MatchTemplate",
                    "template": "Base.png",
                    "next": ["N", "#self"]
                },
                "N": {"next": []},
                "P": {"next": []},
                "P@N": {"next": []},
                "Child": {"baseTask": "Base", "action": "ClickSelf"},
                "P@Base": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let graph = compile_maa_task_graph(temp.path()).unwrap();
        let inherited = graph.task_facts("Child").unwrap();
        assert_eq!(
            inherited.algorithm.as_ref().unwrap().origin,
            MaaFactOrigin::Inherited
        );
        assert_eq!(
            inherited.action.as_ref().unwrap().origin,
            MaaFactOrigin::Declared
        );
        assert_eq!(
            inherited.template.as_ref().unwrap().origin,
            MaaFactOrigin::MaaDefaulted
        );
        assert_eq!(
            inherited.topology["next"][0].origin,
            MaaFactOrigin::Inherited
        );
        assert_eq!(
            inherited.topology["next"][1].origin,
            MaaFactOrigin::Composed
        );

        let composed = graph.task_facts("P@Base").unwrap();
        assert_eq!(
            composed.template.as_ref().unwrap().origin,
            MaaFactOrigin::MaaDefaulted
        );
        assert!(
            composed.topology["next"]
                .iter()
                .all(|fact| fact.origin == MaaFactOrigin::Composed)
        );
    }

    #[test]
    fn bounded_intake_rejects_root_symlink_depth_files_bytes_and_task_limits() {
        let root_file = tempfile::NamedTempFile::new().unwrap();
        let err = compile_maa_task_graph(root_file.path()).unwrap_err();
        assert!(err.message.contains("must be a directory"));

        #[cfg(unix)]
        {
            let root = tempfile::TempDir::new().unwrap();
            let target = tempfile::TempDir::new().unwrap();
            std::os::unix::fs::symlink(target.path(), root.path().join("linked")).unwrap();
            let err = compile_maa_task_graph(root.path()).unwrap_err();
            assert!(err.message.contains("symbolic link"));
        }

        let depth_root = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(depth_root.path().join("one/two")).unwrap();
        let err = compile_maa_task_graph_with_limits(
            depth_root.path(),
            MaaIntakeLimits {
                max_directory_depth: 1,
                ..MaaIntakeLimits::PRODUCTION
            },
        )
        .unwrap_err();
        assert!(err.message.contains("directory depth exceeds 1"));

        let count_root = tempfile::TempDir::new().unwrap();
        fs::write(count_root.path().join("a.json"), b"{}").unwrap();
        fs::write(count_root.path().join("b.json"), b"{}").unwrap();
        let err = compile_maa_task_graph_with_limits(
            count_root.path(),
            MaaIntakeLimits {
                max_json_files: 1,
                ..MaaIntakeLimits::PRODUCTION
            },
        )
        .unwrap_err();
        assert!(err.message.contains("JSON file count exceeds 1"));

        let file_bytes_root = tempfile::TempDir::new().unwrap();
        fs::write(file_bytes_root.path().join("tasks.json"), b"{}").unwrap();
        let err = compile_maa_task_graph_with_limits(
            file_bytes_root.path(),
            MaaIntakeLimits {
                max_json_file_bytes: 1,
                ..MaaIntakeLimits::PRODUCTION
            },
        )
        .unwrap_err();
        assert!(err.message.contains("JSON file bytes exceed 1"));

        let aggregate_root = tempfile::TempDir::new().unwrap();
        fs::write(aggregate_root.path().join("a.json"), b"{}").unwrap();
        fs::write(aggregate_root.path().join("b.json"), b"{}").unwrap();
        let err = compile_maa_task_graph_with_limits(
            aggregate_root.path(),
            MaaIntakeLimits {
                max_aggregate_json_bytes: 3,
                ..MaaIntakeLimits::PRODUCTION
            },
        )
        .unwrap_err();
        assert!(err.message.contains("aggregate JSON bytes exceed 3"));

        let task_root = tempfile::TempDir::new().unwrap();
        fs::write(
            task_root.path().join("tasks.json"),
            serde_json::to_vec(&json!({"A": {}, "B": {}})).unwrap(),
        )
        .unwrap();
        let err = compile_maa_task_graph_with_limits(
            task_root.path(),
            MaaIntakeLimits {
                max_raw_tasks: 1,
                ..MaaIntakeLimits::PRODUCTION
            },
        )
        .unwrap_err();
        assert!(err.message.contains("raw task count exceeds 1"));
    }

    #[test]
    fn malformed_core_typed_fields_fail_loudly() {
        let malformed = [
            ("baseTask", json!({"A": {"baseTask": 7}})),
            ("Doc", json!({"A": {"Doc": []}})),
            ("algorithm", json!({"A": {"algorithm": false}})),
            ("action", json!({"A": {"action": 2}})),
            ("template", json!({"A": {"template": {}}})),
            ("roi", json!({"A": {"roi": [0, 1, 2]}})),
            ("rectMove", json!({"A": {"rectMove": [0, 1, 2, "3"]}})),
        ];
        for (field, value) in malformed {
            let err = compile_maa_task_graph_from_value(value).unwrap_err();
            assert!(
                err.message.contains(field),
                "field={field}, error={}",
                err.message
            );
        }
    }

    fn at_chain_name(components: usize) -> String {
        let mut parts = (1..components)
            .map(|index| format!("P{index}"))
            .collect::<Vec<_>>();
        parts.push("Base".to_string());
        parts.join("@")
    }
}
