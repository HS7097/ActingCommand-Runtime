// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedAdbEndpoint, ResolvedInstanceEndpoint,
};
use serde_json::json;
use std::path::Path;

const CHECK_CONFIG_SCHEMA_VERSION: &str = "actingcommand.actingd.check-config.v1";
/// Inputs this command cannot validate: the vision provider manifest is only read and
/// validated inside host startup, and nothing under `state_root` is inspected here.
const NOT_CHECKED: [&str; 2] = ["vision_provider_manifest", "state_root"];

/// Loads, assembles and validates a configuration exactly as startup would, then stops
/// before the first side effect: nothing under `state_root` is created, read or locked,
/// no ledger is opened and no socket is bound.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), ActingdError> {
    if arguments.len() > 3 {
        return Err(ActingdError::config("check_config_usage_invalid"));
    }
    let mut config = None;
    let (pairs, remaining) = arguments[1..].as_chunks::<2>();
    for pair in pairs {
        if pair[0] != "--config" || config.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config("check_config_option_invalid"));
        }
        config = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config("check_config_option_invalid"));
    }
    let config_path = config.ok_or_else(|| ActingdError::config("check_config_config_missing"))?;
    let checked = config::load(&config_path)
        .map_err(|code| (code, "load"))
        .and_then(|file| file.assemble().map_err(|code| (code, "assemble")))
        .and_then(|assembly| {
            assembly
                .host
                .validate()
                .map_err(|error| (error.code(), "validate"))?;
            summarize(&config_path, &assembly)
        });
    let (report, result) = match checked {
        Ok(report) => (report, Ok(())),
        Err((code, stage)) => (
            json!({
                "schema_version": CHECK_CONFIG_SCHEMA_VERSION,
                "status": "failed",
                "error": { "code": code, "stage": stage },
            }),
            Err(ActingdError::config(code)),
        ),
    };
    let encoded = serde_json::to_string(&report)
        .map_err(|_| ActingdError::process("check_config_report_encode_failed"))?;
    println!("{encoded}");
    result
}

fn summarize(
    config_path: &Path,
    assembly: &RuntimeAssembly,
) -> Result<serde_json::Value, (&'static str, &'static str)> {
    let registry = &assembly.registry;
    let instances = registry
        .instance_aliases()
        .into_iter()
        .map(|alias| {
            let mode = match registry.mode_for_alias(&alias) {
                Some(ScheduledExecutionMode::DeviceRegistry) => "device_registry",
                Some(ScheduledExecutionMode::FixtureSimulation) => "fixture_simulation",
                None => return Err(("execution_backend_registry_incomplete", "validate")),
            };
            // The startup package as assembled: locator and digest, neither opened nor hashed.
            let startup_package = assembly.host.startup_packages().get(&alias).map(|request| {
                json!({
                    "package": request.package_path(),
                    "expected_sha256": request.expected_sha256(),
                })
            });
            if let Some(key) = registry.deferred_binding(&alias) {
                // Bound at startup by one MuMuManager discovery run; nothing is spawned here.
                return Ok(json!({
                    "alias": alias,
                    "mode": mode,
                    "binding": "discovery_pending",
                    "instance_index": key.index(),
                    "instance_name": key.name(),
                    "startup_package": startup_package,
                }));
            }
            let resolved = registry
                .resolve(&alias)
                .ok_or(("execution_backend_registry_incomplete", "validate"))?;
            let endpoint = resolved
                .adb_endpoint()
                .and_then(ResolvedInstanceEndpoint::bound);
            Ok(json!({
                "alias": alias,
                "mode": mode,
                "binding": "explicit",
                "adb_host": endpoint.map(ResolvedAdbEndpoint::host),
                "adb_port": endpoint.map(ResolvedAdbEndpoint::port),
                "startup_package": startup_package,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bind_address = assembly.host.bind_address();
    Ok(json!({
        "schema_version": CHECK_CONFIG_SCHEMA_VERSION,
        "status": "ok",
        "config_path": config_path.to_string_lossy(),
        "state_root": assembly.host.state_root().to_string_lossy(),
        "bind_host": bind_address.ip().to_string(),
        "bind_port": bind_address.port(),
        "instance_count": instances.len(),
        "instances": instances,
        "policy_configured": assembly.policy.is_some(),
        "config_manifest": assembly.manifest,
        "not_checked": NOT_CHECKED,
    }))
}
