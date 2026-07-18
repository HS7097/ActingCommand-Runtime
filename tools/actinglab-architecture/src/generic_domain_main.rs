// SPDX-License-Identifier: AGPL-3.0-only

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use actingcommand_actinglab_architecture::generic_domain::{
    GENERIC_DOMAIN_REGISTRY_PATH, load_generic_domain_registry, validate_workspace_genericity,
    workspace_surface_snapshot,
};

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
    let root = workspace_root()?;
    match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [flag] if flag == "--check" => {
            let registry_path = root.join(GENERIC_DOMAIN_REGISTRY_PATH);
            let registry = load_generic_domain_registry(&registry_path)?;
            validate_workspace_genericity(&root, &registry)?;
            println!("generic-domain registry and identity guard match protected Runtime surfaces");
            Ok(())
        }
        [flag] if flag == "--snapshot" => {
            let snapshot = workspace_surface_snapshot(&root)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&snapshot)
                    .map_err(|error| format!("failed to serialize surface snapshot: {error}"))?
            );
            Ok(())
        }
        _ => Err("usage: generic-domain-guard <--check|--snapshot>".to_string()),
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "architecture tool must live at tools/<name>".to_string())
}
