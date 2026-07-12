// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, FlagArgs, runtime_state_root};
use actingcommand_contract::{
    EventActor, EventSource, PackageDebugRequest, ProjectionProfile, RuntimeResult,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs;

pub(super) fn run_package_debug(args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    flags.expect_positionals("lab debug-package", 0)?;
    let package = flags.required_path("--zip")?;
    let package = fs::canonicalize(&package).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to resolve debug package {}: {error}",
            package.display()
        ))
    })?;
    let expected = flags
        .optional("--expected-sha256")
        .filter(|value| value != "true")
        .ok_or_else(|| {
            CliError::usage(
                "lab debug-package requires --expected-sha256 <sha256> from an external trust source",
            )
        })?;
    let expected = Sha256Hash::parse_hex(&expected)
        .map_err(|error| CliError::package_invalid(error.to_string()))?;
    let request =
        PackageDebugRequest::new(package.to_string_lossy().into_owned(), expected.to_string())
            .map_err(|error| CliError::usage(error.to_string()))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        runtime_state_root()?,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(|error| CliError::device(error.to_string()))?;
    let session = client
        .begin_debug_session()
        .map_err(|error| CliError::device(error.to_string()))?;
    let receipt = session
        .debug_package(request)
        .map_err(|error| CliError::device(error.to_string()))?;
    let summary = match receipt.result() {
        Some(RuntimeResult::PackageDebugCompleted { summary }) => summary,
        _ => {
            return Err(CliError::device(
                "Runtime returned an invalid package debug receipt",
            ));
        }
    };
    let events = session
        .query_events(ProjectionProfile::Lab)
        .map_err(|error| CliError::device(error.to_string()))?;
    Ok(json!({
        "schema_version": "actingcommand.lab.package-debug.v1",
        "authority": "runtime",
        "correlation_id": session.correlation_id(),
        "summary": summary,
        "terminal_receipt": receipt,
        "events": events
    }))
}
