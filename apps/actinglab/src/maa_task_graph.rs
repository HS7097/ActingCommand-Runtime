// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, ResolvedResourceRoot};
use actingcommand_lab::MaaTaskCompileRequest;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn run_resource_maa_task_compile(
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let tasks_root = flags
        .optional_path("--maa-tasks")
        .unwrap_or_else(|| default_maa_tasks_root(resource_root));
    let request = MaaTaskCompileRequest {
        tasks_root,
        repo: resource_root.input.clone(),
        resource_root: resource_root.root.clone(),
        resource_layout: resource_root.layout.to_string(),
        selected_task: flags.optional("--task").filter(|value| value != "true"),
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serde_json::to_value(lab.compile_maa_tasks(request)?)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
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
