// SPDX-License-Identifier: AGPL-3.0-only

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use actingcommand_actinglab_architecture::extract_command_inventory;
use serde_json::{Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FATAL: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if !(arguments.is_empty() || matches!(arguments.as_slice(), [flag] if flag == "--check")) {
        return Err("usage: actinglab-command-inventory [--check]".to_string());
    }

    let root = workspace_root()?;
    let inventory = inventory_from_workspace(&root)?;
    if arguments.is_empty() {
        let output = json!({
            "schema_version": "actingcommand.command-inventory.v1",
            "source": "apps/actinglab/src/**/*.rs",
            "dispatch_function": "execute",
            "denominator_kind": "top_level_dispatch_arm",
            "dispatch_arm_count": inventory.dispatch_arm_count,
            "dispatch_arms": inventory.dispatch_arms,
            "command_count": inventory.commands.len(),
            "commands": inventory.commands,
            "pipeline_exemptions": []
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .map_err(|err| format!("failed to serialize command inventory: {err}"))?
        );
        return Ok(());
    }

    let snapshot_path = root.join("ratchet/actinglab_commands.json");
    let snapshot_text = fs::read_to_string(&snapshot_path)
        .map_err(|err| format!("failed to read {}: {err}", snapshot_path.display()))?;
    let snapshot: Value = serde_json::from_str(&snapshot_text)
        .map_err(|err| format!("failed to parse {}: {err}", snapshot_path.display()))?;
    check_snapshot(
        &snapshot,
        &inventory.dispatch_arms,
        &inventory.commands,
        inventory.dispatch_arm_count,
    )?;
    println!(
        "command inventory matches {} commands across {} dispatch arms",
        inventory.commands.len(),
        inventory.dispatch_arm_count
    );
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "architecture tool must live at tools/<name>".to_string())
}

fn inventory_from_workspace(
    root: &Path,
) -> Result<actingcommand_actinglab_architecture::CommandInventory, String> {
    let mut paths = Vec::new();
    collect_rust_files(&root.join("apps/actinglab/src"), &mut paths)?;
    paths.sort();
    let owned_sources = paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
            let source = fs::read_to_string(path)
                .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
            Ok((relative, source))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let sources = owned_sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    extract_command_inventory(&sources)
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(root)
        .map_err(|err| format!("failed to read directory {}: {err}", root.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|err| format!("failed to read directory entry: {err}"))?
            .path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn check_snapshot(
    snapshot: &Value,
    dispatch_arms: &[String],
    commands: &[String],
    dispatch_arm_count: usize,
) -> Result<(), String> {
    if snapshot["schema_version"] != "actingcommand.command-inventory.v1" {
        return Err("command snapshot has an unsupported schema_version".to_string());
    }
    if snapshot["dispatch_arm_count"].as_u64() != Some(dispatch_arm_count as u64) {
        return Err("command snapshot dispatch_arm_count is stale".to_string());
    }
    let snapshot_dispatch_arms = string_array(snapshot, "dispatch_arms")?;
    if snapshot_dispatch_arms != dispatch_arms {
        return Err("command snapshot dispatch_arms are stale".to_string());
    }
    if snapshot["command_count"].as_u64() != Some(commands.len() as u64) {
        return Err("command snapshot command_count is stale".to_string());
    }
    let snapshot_commands = string_array(snapshot, "commands")?;
    if snapshot_commands != commands {
        return Err("command snapshot commands are stale".to_string());
    }
    Ok(())
}

fn string_array(snapshot: &Value, field: &str) -> Result<Vec<String>, String> {
    snapshot[field]
        .as_array()
        .ok_or_else(|| format!("command snapshot {field} must be an array"))?
        .iter()
        .map(|command| {
            command
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("command snapshot {field} contains a non-string value"))
        })
        .collect()
}
