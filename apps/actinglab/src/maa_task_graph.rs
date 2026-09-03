// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, ResolvedResourceRoot};
use actingcommand_lab::{JsonDocument, MaaTaskFacts, compile_maa_task_graph};
use serde::Serialize;
use serde_json::Value;

const MAX_MAA_MULTI_TASK_FACTS_DATA_BYTES: usize = 16_777_216;

pub(super) fn run_resource_maa_task_compile(
    flags: &FlagArgs,
    resource_root: &ResolvedResourceRoot,
) -> CliOutcome<Value> {
    let tasks_root = flags
        .optional_path("--maa-tasks")
        .ok_or_else(|| CliError::usage("resource compile-maa requires --maa-tasks <dir>"))?;
    let facts_mode = flags.bool("--facts");
    let task_ids = flags.values("--task");
    if facts_mode && task_ids.iter().any(|task_id| task_id == "true") {
        return Err(CliError::usage(
            "resource compile-maa --facts requires each --task <id>",
        ));
    }
    if !facts_mode && task_ids.len() > 1 {
        return Err(CliError::usage(
            "resource compile-maa accepts repeated --task only with --facts",
        ));
    }
    let graph = compile_maa_task_graph(&tasks_root)?;
    let stats = graph.stats();
    if facts_mode {
        let mut tasks = graph.task_facts_many(&task_ids)?;
        if tasks.len() > 1 {
            return bounded_maa_task_facts_set(tasks);
        }
        let task = tasks.pop().expect("bounded selection is non-empty");
        return serde_json::to_value(MaaTaskFactsResponse {
            schema_version: "actingcommand.maa-task-facts.v1",
            task,
        })
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")));
    }
    let selected_task = task_ids
        .first()
        .filter(|value| value.as_str() != "true")
        .map(|task_id| graph.task_document(task_id))
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

fn bounded_maa_task_facts_set(tasks: Vec<MaaTaskFacts>) -> CliOutcome<Value> {
    let bytes = serde_json::to_vec(&MaaTaskFactsSetResponse {
        schema_version: "actingcommand.maa-task-facts-set.v1",
        tasks,
    })
    .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))?;
    if bytes.len() > MAX_MAA_MULTI_TASK_FACTS_DATA_BYTES {
        return Err(CliError::package_invalid(format!(
            "serialized MAA multi-task facts data exceeds {MAX_MAA_MULTI_TASK_FACTS_DATA_BYTES} UTF-8 bytes"
        )));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| CliError::device(format!("failed to decode Lab response: {error}")))
}

#[derive(Serialize)]
struct MaaTaskFactsResponse {
    schema_version: &'static str,
    task: MaaTaskFacts,
}

#[derive(Serialize)]
struct MaaTaskFactsSetResponse {
    schema_version: &'static str,
    tasks: Vec<MaaTaskFacts>,
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
    use actingcommand_lab::MaaFactValue;
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
    fn multi_task_facts_fail_atomically_on_empty_duplicate_missing_and_bounds() {
        let root = tempfile::TempDir::new().unwrap();
        let tasks_root = root.path().join("maa");
        fs::create_dir_all(&tasks_root).unwrap();
        fs::write(
            tasks_root.join("tasks.json"),
            serde_json::to_vec(&serde_json::json!({"Alpha": {}, "Zulu": {}})).unwrap(),
        )
        .unwrap();
        let graph = compile_maa_task_graph(&tasks_root).unwrap();

        let empty = graph.task_facts_many(&[]).unwrap_err();
        assert_eq!(empty.code, "validation_failed");
        assert!(empty.message.contains("1..=256"));

        let too_many = (0..=256)
            .map(|index| format!("Task{index}"))
            .collect::<Vec<_>>();
        let cardinality = graph.task_facts_many(&too_many).unwrap_err();
        assert_eq!(cardinality.code, "validation_failed");
        assert!(cardinality.message.contains("1..=256"));

        let selection_bytes = graph.task_facts_many(&["x".repeat(65_537)]).unwrap_err();
        assert_eq!(selection_bytes.code, "validation_failed");
        assert!(selection_bytes.message.contains("65536"));

        let duplicate = graph
            .task_facts_many(&["Zulu".to_string(), "Alpha".to_string(), "Alpha".to_string()])
            .unwrap_err();
        assert_eq!(duplicate.code, "validation_failed");
        assert!(duplicate.message.contains("'Alpha'"));

        let missing = graph
            .task_facts_many(&["Missing-Z".to_string(), "Missing-A".to_string()])
            .unwrap_err();
        assert_eq!(missing.code, "package_invalid");
        assert!(missing.message.contains("'Missing-A'"));

        let mut oversized = graph.task_facts("Alpha").unwrap();
        let mut oversized_doc = oversized.task_id.clone();
        oversized_doc.value = MaaFactValue::String {
            value: "x".repeat(MAX_MAA_MULTI_TASK_FACTS_DATA_BYTES),
        };
        oversized.doc = Some(oversized_doc);
        let serialized =
            bounded_maa_task_facts_set(vec![oversized, graph.task_facts("Zulu").unwrap()])
                .unwrap_err();
        assert_eq!(serialized.code, "package_invalid");
        assert!(serialized.message.contains("16777216"));
    }

    #[test]
    fn facts_mode_emits_multi_task_set_and_preserves_single_task_v1() {
        let root = tempfile::TempDir::new().unwrap();
        let tasks_root = root.path().join("maa");
        fs::create_dir_all(&tasks_root).unwrap();
        fs::write(
            tasks_root.join("tasks.json"),
            serde_json::to_vec(&serde_json::json!({
                "Alpha": {"action": "DoNothing"},
                "Zulu": {"algorithm": "JustReturn"}
            }))
            .unwrap(),
        )
        .unwrap();
        let resource_root = ResolvedResourceRoot {
            input: root.path().to_path_buf(),
            root: root.path().to_path_buf(),
            layout: "unresolved",
        };

        let multi = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            tasks_root.display().to_string(),
            "--task".to_string(),
            "Zulu".to_string(),
            "--task".to_string(),
            "Alpha".to_string(),
            "--facts".to_string(),
        ])
        .unwrap();
        let output = run_resource_maa_task_compile(&multi, &resource_root).unwrap();
        assert_eq!(
            output.pointer("/schema_version").and_then(Value::as_str),
            Some("actingcommand.maa-task-facts-set.v1")
        );
        assert_eq!(
            output
                .pointer("/tasks/0/task_id/value")
                .and_then(Value::as_str),
            Some("Alpha")
        );
        assert_eq!(
            output
                .pointer("/tasks/1/task_id/value")
                .and_then(Value::as_str),
            Some("Zulu")
        );
        assert!(output.get("task").is_none());

        let single = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            tasks_root.display().to_string(),
            "--task".to_string(),
            "Zulu".to_string(),
            "--facts".to_string(),
        ])
        .unwrap();
        let output = run_resource_maa_task_compile(&single, &resource_root).unwrap();
        assert_eq!(
            output.pointer("/schema_version").and_then(Value::as_str),
            Some("actingcommand.maa-task-facts.v1")
        );
        assert_eq!(
            output
                .pointer("/task/task_id/value")
                .and_then(Value::as_str),
            Some("Zulu")
        );
        assert!(output.get("tasks").is_none());

        let repeated_without_facts = FlagArgs::parse(&[
            "--maa-tasks".to_string(),
            tasks_root.display().to_string(),
            "--task".to_string(),
            "Alpha".to_string(),
            "--task".to_string(),
            "Zulu".to_string(),
        ])
        .unwrap();
        let error =
            run_resource_maa_task_compile(&repeated_without_facts, &resource_root).unwrap_err();
        assert_eq!(error.code, "validation_failed");
        assert!(error.message.contains("only with --facts"));
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
