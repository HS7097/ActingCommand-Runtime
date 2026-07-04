// SPDX-License-Identifier: AGPL-3.0-only

//! MAA task graph expansion at the resource-data boundary.
//!
//! This module consumes MAA task JSON data and implements the public task-schema
//! semantics needed before ActingCommand can convert those resources into its own
//! schema. It does not call or copy the upstream MAA engine.

use super::{CliError, CliOutcome, FlagArgs, ResolvedResourceRoot};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const LIST_FIELDS: [&str; 5] = [
    "sub",
    "next",
    "onErrorNext",
    "exceededNext",
    "reduceOtherTimes",
];

#[derive(Debug, Clone)]
pub(super) struct MaaTaskGraph {
    tasks: BTreeMap<String, Value>,
    stats: MaaTaskGraphStats,
}

impl MaaTaskGraph {
    pub(super) fn task(&self, task_id: &str) -> Option<&Value> {
        self.tasks.get(task_id)
    }

    pub(super) fn tasks(&self) -> &BTreeMap<String, Value> {
        &self.tasks
    }

    pub(super) fn to_summary_json(&self) -> Value {
        json!({
            "schema_version": "actingcommand.maa-task-graph.v1",
            "source_files": self.stats.source_files,
            "raw_tasks": self.stats.raw_tasks,
            "compiled_tasks": self.stats.compiled_tasks,
            "base_task_derivations": self.stats.base_task_derivations,
            "explicit_at_tasks": self.stats.explicit_at_tasks,
            "implicit_at_tasks": self.stats.implicit_at_tasks,
            "virtual_references": self.stats.virtual_references,
            "task_ids": self.tasks.keys().cloned().collect::<Vec<_>>()
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MaaTaskGraphStats {
    pub(super) source_files: usize,
    pub(super) raw_tasks: usize,
    pub(super) compiled_tasks: usize,
    pub(super) base_task_derivations: usize,
    pub(super) explicit_at_tasks: usize,
    pub(super) implicit_at_tasks: usize,
    pub(super) virtual_references: usize,
}

pub(super) fn compile_maa_task_graph_family(tasks_root: &Path) -> CliOutcome<MaaTaskGraph> {
    let mut files = collect_maa_task_files(tasks_root)?;
    if files.is_empty() {
        return Err(CliError::package_invalid(format!(
            "no MAA task JSON files found under {}",
            tasks_root.display()
        )));
    }
    files.sort();
    if let Some(index) = files
        .iter()
        .position(|path| path.file_name().and_then(|name| name.to_str()) == Some("tasks.json"))
    {
        let root_tasks = files.remove(index);
        files.insert(0, root_tasks);
    }

    let mut registry = MaaRawTaskRegistry::default();
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

pub(super) fn run_resource_maa_task_compile(
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let tasks_root = flags
        .optional_path("--maa-tasks")
        .unwrap_or_else(|| default_maa_tasks_root(resource_root));
    let graph = compile_maa_task_graph_family(&tasks_root)?;
    let mut summary = graph.to_summary_json();
    let object = summary
        .as_object_mut()
        .expect("MAA task graph summary is object");
    object.insert(
        "repo".to_string(),
        Value::String(resource_root.input.display().to_string()),
    );
    object.insert(
        "resource_root".to_string(),
        Value::String(resource_root.root.display().to_string()),
    );
    object.insert(
        "resource_layout".to_string(),
        Value::String(resource_root.layout.to_string()),
    );
    object.insert(
        "maa_tasks_root".to_string(),
        Value::String(tasks_root.display().to_string()),
    );
    if let Some(task_id) = flags.optional("--task").filter(|value| value != "true") {
        let task = graph.task(&task_id).ok_or_else(|| {
            CliError::package_invalid(format!("compiled MAA task '{task_id}' was not found"))
        })?;
        object.insert("selected_task".to_string(), task.clone());
    }
    Ok(summary)
}

#[derive(Debug, Default)]
struct MaaRawTaskRegistry {
    tasks: BTreeMap<String, RawMaaTask>,
}

#[derive(Debug, Clone)]
struct RawMaaTask {
    task_id: String,
    data: Map<String, Value>,
    source: String,
}

impl MaaRawTaskRegistry {
    fn load_file(&mut self, path: &Path) -> CliOutcome<()> {
        let text = fs::read_to_string(path).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to read MAA task file {}: {err}",
                path.display()
            ))
        })?;
        let value = serde_json::from_str::<Value>(&text).map_err(|err| {
            CliError::package_invalid(format!(
                "failed to parse MAA task file {}: {err}",
                path.display()
            ))
        })?;
        self.load_value(&path.display().to_string(), value)
    }

    fn load_value(&mut self, source: &str, value: Value) -> CliOutcome<()> {
        let object = value.as_object().ok_or_else(|| {
            CliError::package_invalid(format!("MAA task source {source} must be a JSON object"))
        })?;
        for (task_id, task_value) in object {
            let data = task_value.as_object().cloned().ok_or_else(|| {
                CliError::package_invalid(format!(
                    "MAA task '{task_id}' in {source} must be a JSON object"
                ))
            })?;
            self.insert_task(RawMaaTask {
                task_id: task_id.clone(),
                data,
                source: source.to_string(),
            });
        }
        Ok(())
    }

    fn insert_task(&mut self, task: RawMaaTask) {
        let Some(existing) = self.tasks.get(&task.task_id) else {
            self.tasks.insert(task.task_id.clone(), task);
            return;
        };
        if task.data.contains_key("baseTask") {
            self.tasks.insert(task.task_id.clone(), task);
            return;
        }
        let mut inherited = existing.data.clone();
        merge_object(&mut inherited, &task.data);
        self.tasks.insert(
            task.task_id.clone(),
            RawMaaTask {
                task_id: task.task_id,
                data: inherited,
                source: task.source,
            },
        );
    }
}

struct MaaTaskCompiler {
    raw: BTreeMap<String, RawMaaTask>,
    materialized: HashMap<String, Value>,
    expanded: HashMap<String, Value>,
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
            .flat_map(task_references)
            .filter(|task_id| task_id != "Stop" && !self.expanded.contains_key(task_id))
            .collect::<BTreeSet<_>>();
        for task_id in referenced {
            self.expand_task(&task_id, &mut Vec::new())?;
        }
        let mut tasks = BTreeMap::new();
        for (task_id, task) in self.expanded {
            tasks.insert(task_id, task);
        }
        self.stats.compiled_tasks = tasks.len();
        Ok(MaaTaskGraph {
            tasks,
            stats: self.stats,
        })
    }

    fn expand_task(&mut self, task_id: &str, stack: &mut Vec<String>) -> CliOutcome<Value> {
        if task_id == "Stop" {
            return Ok(json!({"task_id": "Stop", "algorithm": "Stop"}));
        }
        if let Some(task) = self.expanded.get(task_id) {
            return Ok(task.clone());
        }
        let mut task = self.materialize_task(task_id, stack)?;
        for field in LIST_FIELDS {
            let Some(value) = task.get(field).cloned() else {
                continue;
            };
            let expressions = task_list_expressions(&value).ok_or_else(|| {
                CliError::package_invalid(format!(
                    "MAA task '{task_id}' field '{field}' must be a string or string array"
                ))
            })?;
            let expanded = self.expand_expression_list(task_id, field, &expressions)?;
            task.as_object_mut()
                .expect("materialized task is object")
                .insert(
                    field.to_string(),
                    Value::Array(expanded.into_iter().map(Value::String).collect()),
                );
        }
        self.validate_task_references(task_id, &task)?;
        self.expanded.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    fn materialize_task(&mut self, task_id: &str, stack: &mut Vec<String>) -> CliOutcome<Value> {
        if let Some(task) = self.materialized.get(task_id) {
            return Ok(task.clone());
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
                    Some("#none") => Map::new(),
                    Some(base_id) => {
                        self.stats.base_task_derivations += 1;
                        value_object(self.materialize_task(base_id, stack)?, base_id)?
                    }
                    None => match split {
                        Some((prefix, base_id)) => {
                            self.stats.explicit_at_tasks += 1;
                            let base =
                                value_object(self.materialize_task(base_id, stack)?, base_id)?;
                            rebase_task_list_defaults(base, prefix)
                        }
                        None => Map::new(),
                    },
                };
                filter_algorithm_specific_inheritance(&mut base, &raw.data);
                merge_object(&mut base, &raw.data);
                base.remove("baseTask");
                if (base_task.is_some() || is_explicit_at)
                    && !raw.data.contains_key("template")
                    && looks_like_template_task(&base)
                {
                    base.insert(
                        "template".to_string(),
                        Value::String(default_template_name(task_id)),
                    );
                }
                base
            }
            None => {
                let (prefix, base_id) = split.expect("checked split");
                self.stats.implicit_at_tasks += 1;
                let base = value_object(self.materialize_task(base_id, stack)?, base_id)?;
                rebase_task_list_defaults(base, prefix)
            }
        };
        stack.pop();
        task.insert("task_id".to_string(), Value::String(task_id.to_string()));
        let task = Value::Object(task);
        self.materialized.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    fn expand_expression_list(
        &mut self,
        task_id: &str,
        field: &str,
        expressions: &[String],
    ) -> CliOutcome<Vec<String>> {
        let mut out = Vec::new();
        for expression in expressions {
            let mut parser = MaaExpressionParser::new(self, task_id, field, expression);
            merge_unique(&mut out, parser.parse()?);
        }
        Ok(out)
    }

    fn validate_task_references(&mut self, owner: &str, task: &Value) -> CliOutcome<()> {
        let mut errors = Vec::new();
        for field in LIST_FIELDS {
            for target in
                task_list_expressions(task.get(field).unwrap_or(&Value::Null)).unwrap_or_default()
            {
                if !self.can_materialize(&target, &mut Vec::new()) {
                    errors.push(format!("{field} references unresolved task '{target}'"));
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

    fn can_materialize(&mut self, task_id: &str, stack: &mut Vec<String>) -> bool {
        task_id == "Stop" || self.materialize_task(task_id, stack).is_ok()
    }

    fn split_materializable_at_task<'a>(&self, task_id: &'a str) -> Option<(&'a str, &'a str)> {
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
        left: &[String],
        sharp_type: &str,
    ) -> CliOutcome<Vec<String>> {
        self.stats.virtual_references += 1;
        match sharp_type {
            "none" => Ok(Vec::new()),
            "self" => Ok(vec![context_task.to_string()]),
            "back" => Ok(left.to_vec()),
            "next" | "sub" | "on_error_next" | "exceeded_next" | "reduce_other_times" => {
                let field = sharp_field_name(sharp_type);
                let mut out = Vec::new();
                for task_id in left {
                    let task = self.expand_task(task_id, &mut Vec::new())?;
                    let value = task.get(field).cloned().unwrap_or(Value::Null);
                    merge_unique(&mut out, task_list_expressions(&value).unwrap_or_default());
                }
                Ok(out)
            }
            other => Err(CliError::package_invalid(format!(
                "unknown MAA virtual task '#{other}'"
            ))),
        }
    }
}

struct MaaExpressionParser<'a> {
    compiler: &'a mut MaaTaskCompiler,
    context_task: &'a str,
    field: &'a str,
    input: &'a str,
    chars: Vec<char>,
    pos: usize,
}

impl<'a> MaaExpressionParser<'a> {
    fn new(
        compiler: &'a mut MaaTaskCompiler,
        context_task: &'a str,
        field: &'a str,
        input: &'a str,
    ) -> Self {
        Self {
            compiler,
            context_task,
            field,
            input,
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse(&mut self) -> CliOutcome<Vec<String>> {
        let result = self.parse_union_diff()?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(self.error("unexpected trailing input"));
        }
        Ok(result)
    }

    fn parse_union_diff(&mut self) -> CliOutcome<Vec<String>> {
        let mut left = self.parse_repeat()?;
        loop {
            self.skip_ws();
            if self.consume('+') {
                let right = self.parse_repeat()?;
                merge_unique(&mut left, right);
            } else if self.consume('^') {
                let right = self.parse_repeat()?;
                let banned = right.into_iter().collect::<HashSet<_>>();
                left.retain(|item| !banned.contains(item));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_repeat(&mut self) -> CliOutcome<Vec<String>> {
        let mut value = self.parse_at_sharp()?;
        self.skip_ws();
        if self.consume('*') {
            let count = self.parse_usize()?;
            let original = value.clone();
            for _ in 1..count {
                value.extend(original.clone());
            }
        }
        Ok(value)
    }

    fn parse_at_sharp(&mut self) -> CliOutcome<Vec<String>> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.consume('@') {
                let right = self.parse_unary()?;
                left = combine_at_tasks(&left, &right);
            } else if self.consume('#') {
                let sharp_type = self.parse_ident()?;
                left = self
                    .compiler
                    .expand_virtual_field(self.context_task, &left, &sharp_type)?;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> CliOutcome<Vec<String>> {
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
            return self
                .compiler
                .expand_virtual_field(self.context_task, &[], &sharp_type);
        }
        let ident = self.parse_ident()?;
        if ident.is_empty() {
            return Err(self.error("expected task id"));
        }
        Ok(vec![ident])
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

fn collect_maa_task_files(root: &Path) -> CliOutcome<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_maa_task_files_inner(root, &mut files)?;
    Ok(files)
}

fn default_maa_tasks_root(resource_root: &ResolvedResourceRoot) -> PathBuf {
    let relative = Path::new("upstream-derived")
        .join("upstream")
        .join("MaaAssistantArknights")
        .join("resource")
        .join("tasks");
    let candidates = [
        resource_root.input.join(&relative),
        resource_root
            .root
            .parent()
            .map(|parent| parent.join(&relative))
            .unwrap_or_else(|| resource_root.root.join(&relative)),
        resource_root.root.join(&relative),
    ];
    candidates
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

fn collect_maa_task_files_inner(root: &Path, files: &mut Vec<PathBuf>) -> CliOutcome<()> {
    let entries = fs::read_dir(root).map_err(|err| {
        CliError::package_invalid(format!(
            "failed to read MAA task directory {}: {err}",
            root.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::package_invalid(format!(
                "failed to read MAA task directory {}: {err}",
                root.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_maa_task_files_inner(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(())
}

fn merge_object(base: &mut Map<String, Value>, child: &Map<String, Value>) {
    for (key, value) in child {
        base.insert(key.clone(), value.clone());
    }
}

fn value_object(value: Value, task_id: &str) -> CliOutcome<Map<String, Value>> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(CliError::package_invalid(format!(
            "MAA task '{task_id}' did not materialize as an object"
        ))),
    }
}

fn rebase_task_list_defaults(mut base: Map<String, Value>, prefix: &str) -> Map<String, Value> {
    for field in LIST_FIELDS {
        let Some(value) = base.get(field).cloned() else {
            continue;
        };
        let Some(expressions) = task_list_expressions(&value) else {
            continue;
        };
        base.insert(
            field.to_string(),
            Value::Array(
                expressions
                    .into_iter()
                    .map(|expression| Value::String(rebase_expression(prefix, &expression)))
                    .collect(),
            ),
        );
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
    for key in [
        "template",
        "templThreshold",
        "maskRange",
        "colorScales",
        "colorWithClose",
        "pureColor",
        "method",
        "text",
        "ocrReplace",
        "fullMatch",
        "isAscii",
        "withoutDet",
        "useRaw",
        "binThreshold",
        "inputText",
        "count",
        "ratio",
        "detector",
    ] {
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

fn task_references(task: &Value) -> Vec<String> {
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

fn merge_unique(out: &mut Vec<String>, values: Vec<String>) {
    let mut seen = out.iter().cloned().collect::<BTreeSet<_>>();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
}

fn combine_at_tasks(left: &[String], right: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for lhs in left {
        for rhs in right {
            out.push(format!("{lhs}@{rhs}"));
        }
    }
    out
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
    fn unresolved_reference_fails_loudly() {
        let err = compile_maa_task_graph_from_value(json!({
            "A": {"next": ["Missing"]}
        }))
        .unwrap_err();

        assert!(err.message.contains("unresolved references"));
    }
}
