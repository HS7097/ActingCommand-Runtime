// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, ResolvedResourceRoot};
use actingcommand_lab::{JsonDocument, compile_maa_task_graph};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn run_resource_maa_task_compile(
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let tasks_root = flags
        .optional_path("--maa-tasks")
        .unwrap_or_else(|| default_maa_tasks_root(resource_root));
    let graph = compile_maa_task_graph(&tasks_root)?;
    let stats = graph.stats();
    let selected_task = flags
        .optional("--task")
        .filter(|value| value != "true")
        .map(|task_id| graph.task_document(&task_id))
        .transpose()?;
    serde_json::to_value(MaaTaskCompileResponse {
        schema_version: "actingcommand.maa-task-graph.v1",
        source_files: stats.source_files,
        raw_tasks: stats.raw_tasks,
        compiled_tasks: stats.compiled_tasks,
        base_task_derivations: stats.base_task_derivations,
        explicit_at_tasks: stats.explicit_at_tasks,
        implicit_at_tasks: stats.implicit_at_tasks,
        virtual_references: stats.virtual_references,
        task_ids: graph.task_ids(),
        repo: resource_root.input.display().to_string(),
        resource_root: resource_root.root.display().to_string(),
        resource_layout: resource_root.layout.to_string(),
        maa_tasks_root: tasks_root.display().to_string(),
        selected_task,
    })
    .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}

#[derive(Serialize)]
struct MaaTaskCompileResponse {
    schema_version: &'static str,
    source_files: usize,
    raw_tasks: usize,
    compiled_tasks: usize,
    base_task_derivations: usize,
    explicit_at_tasks: usize,
    implicit_at_tasks: usize,
    virtual_references: usize,
    task_ids: Vec<String>,
    repo: String,
    resource_root: String,
    resource_layout: String,
    maa_tasks_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_task: Option<JsonDocument>,
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
