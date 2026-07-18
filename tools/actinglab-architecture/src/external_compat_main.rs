// SPDX-License-Identifier: AGPL-3.0-only

use std::path::PathBuf;

use actingcommand_actinglab_architecture::external_compat::audit_external_compat;

fn main() {
    if let Err(error) = run() {
        eprintln!("FATAL: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.as_slice() != ["--check"] {
        return Err("usage: external-compat-guard --check".to_string());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| "architecture tool must live at tools/<name>".to_string())?
        .to_path_buf();
    audit_external_compat(&root)?;
    println!("external-compat manifest matches exact files, provenance, and scopes");
    Ok(())
}
