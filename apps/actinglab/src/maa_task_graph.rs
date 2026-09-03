// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, ResolvedResourceRoot};
use actingcommand_lab::{JsonDocument, MaaTaskFacts, compile_maa_task_graph};
use serde::Serialize;
use serde_json::Value;

pub(super) fn run_resource_maa_task_compile(
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let tasks_root = flags
        .optional_path("--maa-tasks")
        .ok_or_else(|| CliError::usage("resource compile-maa requires --maa-tasks <dir>"))?;
    let facts_task_id = flags
        .bool("--facts")
        .then(|| flags.required("--task"))
        .transpose()?;
    let graph = compile_maa_task_graph(&tasks_root)?;
    let stats = graph.stats();
    if let Some(task_id) = facts_task_id {
        let task = graph.task_facts(&task_id)?;
        return serde_json::to_value(MaaTaskFactsResponse {
            schema_version: "actingcommand.maa-task-facts.v1",
            task,
        })
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")));
    }
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
struct MaaTaskFactsResponse {
    schema_version: &'static str,
    task: MaaTaskFacts,
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;

    #[test]
    fn facts_mode_emits_declared_typed_values_with_relative_source_path_and_hash() {
        let root = tempfile::TempDir::new().unwrap();
        let tasks_root = root.path().join("maa");
        let nested = tasks_root.join("region");
        fs::create_dir_all(&nested).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "Warehouse": {
                "Doc": "warehouse entry",
                "algorithm": "MatchTemplate",
                "action": "ClickSelf",
                "roi": [10, 20, 30, 40],
                "template": "Warehouse.png",
                "templThreshold": 0.91,
                "next": ["Stop"]
            }
        }))
        .unwrap();
        fs::write(nested.join("tasks.json"), &bytes).unwrap();
        let expected_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let flags = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            tasks_root.display().to_string(),
            "--task".to_string(),
            "Warehouse".to_string(),
            "--facts".to_string(),
        ])
        .unwrap();
        let resource_root = ResolvedResourceRoot {
            input: root.path().to_path_buf(),
            root: root.path().to_path_buf(),
            layout: "unresolved",
        };

        let output = run_resource_maa_task_compile(&flags, &resource_root).unwrap();

        assert_eq!(
            output.pointer("/schema_version").and_then(Value::as_str),
            Some("actingcommand.maa-task-facts.v1")
        );
        assert!(output.get("repo").is_none());
        assert!(output.get("resource_root").is_none());
        assert!(output.get("maa_tasks_root").is_none());
        assert_eq!(
            output.pointer("/task/doc/value").and_then(Value::as_str),
            Some("warehouse entry")
        );
        assert_eq!(
            output
                .pointer("/task/task_id/value")
                .and_then(Value::as_str),
            Some("Warehouse")
        );
        assert_eq!(
            output
                .pointer("/task/algorithm/value")
                .and_then(Value::as_str),
            Some("MatchTemplate")
        );
        assert_eq!(
            output.pointer("/task/action/value").and_then(Value::as_str),
            Some("ClickSelf")
        );
        assert_eq!(
            output
                .pointer("/task/template/value")
                .and_then(Value::as_str),
            Some("Warehouse.png")
        );
        assert_eq!(
            output
                .pointer("/task/geometry/roi/items/2/value")
                .and_then(Value::as_i64),
            Some(30)
        );
        assert_eq!(
            output
                .pointer("/task/recognition_parameters/templThreshold/value")
                .and_then(Value::as_f64),
            Some(0.91)
        );
        assert_eq!(
            output
                .pointer("/task/topology/next/0/value")
                .and_then(Value::as_str),
            Some("Stop")
        );
        for pointer in [
            "/task/task_id",
            "/task/doc",
            "/task/algorithm",
            "/task/action",
            "/task/geometry/roi/items/0",
            "/task/template",
            "/task/recognition_parameters/templThreshold",
            "/task/topology/next/0",
        ] {
            let fact = output.pointer(pointer).unwrap();
            assert_eq!(
                fact.get("source_task_id").and_then(Value::as_str),
                Some("Warehouse")
            );
            assert_eq!(
                fact.get("source_json_path").and_then(Value::as_str),
                Some("region/tasks.json")
            );
            assert_eq!(
                fact.get("source_file_sha256").and_then(Value::as_str),
                Some(expected_sha256.as_str())
            );
            assert_eq!(fact.get("origin").and_then(Value::as_str), Some("declared"));
        }
        for pointer in [
            "/task/task_id/kind",
            "/task/doc/kind",
            "/task/algorithm/kind",
            "/task/action/kind",
            "/task/template/kind",
            "/task/topology/next/0/kind",
        ] {
            assert_eq!(
                output.pointer(pointer).and_then(Value::as_str),
                Some("string")
            );
        }
        assert_eq!(
            output
                .pointer("/task/geometry/roi/kind")
                .and_then(Value::as_str),
            Some("array")
        );
        assert_eq!(
            output
                .pointer("/task/recognition_parameters/templThreshold/kind")
                .and_then(Value::as_str),
            Some("number")
        );
    }

    #[test]
    fn compile_maa_without_facts_mode_preserves_legacy_v1_output() {
        let root = tempfile::TempDir::new().unwrap();
        let tasks_root = root.path().join("maa");
        fs::create_dir_all(&tasks_root).unwrap();
        fs::write(
            tasks_root.join("tasks.json"),
            serde_json::to_vec(&serde_json::json!({
                "A": {"algorithm": "MatchTemplate", "next": ["Stop"]}
            }))
            .unwrap(),
        )
        .unwrap();
        let flags = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            tasks_root.display().to_string(),
            "--task".to_string(),
            "A".to_string(),
        ])
        .unwrap();
        let resource_root = ResolvedResourceRoot {
            input: root.path().to_path_buf(),
            root: root.path().to_path_buf(),
            layout: "unresolved",
        };

        let output = run_resource_maa_task_compile(&flags, &resource_root).unwrap();

        assert_eq!(
            output,
            serde_json::json!({
                "schema_version": "actingcommand.maa-task-graph.v1",
                "source_files": 1,
                "raw_tasks": 1,
                "compiled_tasks": 1,
                "base_task_derivations": 0,
                "explicit_at_tasks": 0,
                "implicit_at_tasks": 0,
                "virtual_references": 0,
                "task_ids": ["A"],
                "repo": root.path().display().to_string(),
                "resource_root": root.path().display().to_string(),
                "resource_layout": "unresolved",
                "maa_tasks_root": tasks_root.display().to_string(),
                "selected_task": {
                    "algorithm": "MatchTemplate",
                    "next": ["Stop"],
                    "task_id": "A"
                }
            })
        );
    }
}
