// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs};
use actingcommand_contract::BundleSource;
use actingcommand_lab::{
    DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PackageBundleRequest, PackageDigestRequest,
};
use serde::Serialize;
use serde_json::Value;

pub(super) fn run_digest(flags: &FlagArgs) -> CliOutcome<Value> {
    let request = PackageDigestRequest {
        package: flags.required_path("--package")?,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_digest(request)?)
}

/// Workflow #288 A2b: `package bundle --applications <file> --packs-root <directory> --out
/// <directory>`, with `--source-repository` and `--source-commit` together or not at all.
pub(super) fn run_bundle(flags: &FlagArgs) -> CliOutcome<Value> {
    let maintenance = if flags.flags.contains_key("--maintenance") {
        use std::io::Read;
        let path = flags.required_path("--maintenance")?;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| {
                file.take(DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| CliError::usage(format!("{}: {error}", path.display())))?;
        if bytes.len() > DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES {
            return Err(CliError::usage(
                "--maintenance exceeds the package input byte limit",
            ));
        }
        Some(
            serde_json::from_slice(&bytes)
                .map_err(|error| CliError::usage(format!("{}: {error}", path.display())))?,
        )
    } else {
        None
    };
    let source = match (
        flags.flags.contains_key("--source-repository"),
        flags.flags.contains_key("--source-commit"),
    ) {
        (false, false) => None,
        (true, true) => Some(BundleSource {
            repository: flags.required("--source-repository")?,
            commit: flags.required("--source-commit")?,
        }),
        _ => {
            return Err(CliError::usage(
                "--source-repository and --source-commit are given together or not at all",
            ));
        }
    };
    let request = PackageBundleRequest {
        applications: flags.required_path("--applications")?,
        packs_root: flags.required_path("--packs-root")?,
        out: flags.required_path("--out")?,
        source,
        maintenance,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.package_bundle(request)?)
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
