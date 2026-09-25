// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    InstanceResourcePackage, InstanceResourcePackageKind, RuntimeConfigManifest,
};
use actingcommand_device::{MumuInstallSource, MumuManagerSource, resolve_mumu_manager};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ExecutionBackendRegistry, ResolvedAdbEndpoint,
    ResolvedInstanceEndpoint, RuntimeHostConfig,
};
use config::InstanceBindingKey;
use serde_json::json;
use std::path::Path;

const CHECK_CONFIG_SCHEMA_VERSION: &str = "actingcommand.actingd.check-config.v1";
/// Inputs this command cannot validate: the vision provider manifest is only read and
/// validated inside host startup, and nothing under `state_root` is inspected here.
const NOT_CHECKED: [&str; 2] = ["vision_provider_manifest", "state_root"];
/// Added to `not_checked` when a `resource_package` is a directory: the package loader reads
/// a directory only against a Git source-tree reference, which the field does not carry.
const RESOURCE_PACKAGE_DIRECTORY_NOT_CHECKED: &str = "resource_package_directory_declarations";
/// Added to `not_checked` when no MuMu install root could be resolved.
const MUMU_DISCOVERY_NOT_CHECKED: &str = "mumu_discovery";

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
    let mut rejection = None;
    let checked = config::load(&config_path)
        .map_err(|code| (code, "load"))
        .and_then(|file| file.assemble().map_err(|code| (code, "assemble")))
        .and_then(|assembly| {
            let RuntimeAssembly {
                host,
                provider,
                policy,
                manifest,
                resource_packages,
            } = assembly;
            let modes = provider.modes();
            let deferred = provider.deferred_bindings();
            let mumu_root = provider.mumu_root().map(Path::to_path_buf);
            // Registered exactly as startup registers, short of discovery: the registry's own
            // refusals (duplicate aliases or instance ids) carry the code startup would report.
            let registry = provider
                .into_registry()
                .map_err(|error| (error.code(), "assemble"))?;
            host.validate()
                .map_err(|error| (error.code(), "validate"))?;
            let resource_packages = config::validate_resource_packages(&resource_packages)
                .map_err(|refused| {
                    let code = refused.code;
                    rejection = Some(refused);
                    (code, "resource_package")
                })?;
            let checked = CheckedAssembly {
                host,
                registry,
                modes,
                deferred,
                mumu_root,
                policy_configured: policy.is_some(),
                manifest,
            };
            summarize(&config_path, &checked, &resource_packages)
        });
    let (report, result) = match checked {
        Ok(report) => (report, Ok(())),
        Err((code, stage)) => {
            let mut error = json!({ "code": code, "stage": stage });
            let mut failure = ActingdError::config(code);
            if let Some(rejection) = rejection {
                error["detail"] = rejection.detail();
                failure = failure.with_detail(rejection.to_string());
            }
            (
                json!({
                    "schema_version": CHECK_CONFIG_SCHEMA_VERSION,
                    "status": "failed",
                    "error": error,
                }),
                Err(failure),
            )
        }
    };
    let encoded = serde_json::to_string(&report)
        .map_err(|_| ActingdError::process("check_config_report_encode_failed"))?;
    println!("{encoded}");
    result
}

/// Everything `summarize` reports once every check passed: the registry as configured and
/// the declarations only startup can complete.
struct CheckedAssembly {
    host: RuntimeHostConfig,
    registry: ExecutionBackendRegistry,
    modes: BTreeMap<String, ScheduledExecutionMode>,
    deferred: BTreeMap<String, InstanceBindingKey>,
    mumu_root: Option<PathBuf>,
    policy_configured: bool,
    manifest: RuntimeConfigManifest,
}

fn summarize(
    config_path: &Path,
    checked: &CheckedAssembly,
    resource_packages: &BTreeMap<String, InstanceResourcePackage>,
) -> Result<serde_json::Value, (&'static str, &'static str)> {
    let registry = &checked.registry;
    let mut instances = registry
        .instance_aliases()
        .into_iter()
        .map(|alias| {
            let mode = match checked.modes.get(&alias) {
                Some(ScheduledExecutionMode::DeviceRegistry) => "device_registry",
                Some(ScheduledExecutionMode::FixtureSimulation) => "fixture_simulation",
                None => return Err(("execution_backend_registry_incomplete", "validate")),
            };
            // The startup package as assembled: locator and digest, neither opened nor hashed.
            let startup_package = checked.host.startup_packages().get(&alias).map(|request| {
                json!({
                    "package": request.package_path(),
                    "expected_sha256": request.expected_sha256(),
                })
            });
            // The effective stuck-recovery ladder settings (slice #316-B4).
            let stuck_recovery = checked
                .host
                .stuck_recovery()
                .get(&alias)
                .copied()
                .unwrap_or_default();
            if let Some(key) = checked.deferred.get(&alias) {
                // Bound at startup by one MuMuManager discovery run; nothing is spawned here.
                return Ok(json!({
                    "alias": alias,
                    "mode": mode,
                    "binding": "discovery_pending",
                    "instance_index": key.index(),
                    "instance_name": key.name(),
                    "startup_package": startup_package,
                    "stuck_recovery": stuck_recovery.enabled,
                    "stuck_recovery_cooldown_secs": stuck_recovery.cooldown_secs,
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
                "stuck_recovery": stuck_recovery.enabled,
                "stuck_recovery_cooldown_secs": stuck_recovery.cooldown_secs,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // The admitted default resource package, only on an instance that declares one.
    for entry in &mut instances {
        if let Some(package) = entry["alias"]
            .as_str()
            .and_then(|alias| resource_packages.get(alias))
        {
            entry["resource_package"] = json!(package);
        }
    }
    let mut not_checked = NOT_CHECKED.to_vec();
    if resource_packages
        .values()
        .any(|package| package.kind == InstanceResourcePackageKind::Directory)
    {
        not_checked.push(RESOURCE_PACKAGE_DIRECTORY_NOT_CHECKED);
    }
    let (mumu_root, mumu_root_unresolved) = mumu_root_report(checked.mumu_root.as_deref());
    if mumu_root_unresolved.is_some() {
        not_checked.push(MUMU_DISCOVERY_NOT_CHECKED);
    }
    let bind_address = checked.host.bind_address();
    let mut report = json!({
        "schema_version": CHECK_CONFIG_SCHEMA_VERSION,
        "status": "ok",
        "config_path": config_path.to_string_lossy(),
        "state_root": checked.host.state_root().to_string_lossy(),
        "bind_host": bind_address.ip().to_string(),
        "bind_port": bind_address.port(),
        "instance_count": instances.len(),
        "instances": instances,
        "policy_configured": checked.policy_configured,
        "config_manifest": checked.manifest,
        "not_checked": not_checked,
        "mumu_root": mumu_root,
    });
    if let Some(unresolved) = mumu_root_unresolved {
        report["mumu_root_unresolved"] = unresolved;
    }
    Ok(report)
}

/// The MuMu install root the daemon would use: the configured `mumu_root`, else one
/// read-only `resolve_mumu_manager(None)` run (environment, running process, uninstall
/// registry, vendor folders). `MuMuManager.exe` is never run; no instance is started or
/// stopped. A failed resolution is reported as `null` plus its reason, never refused.
fn mumu_root_report(configured: Option<&Path>) -> (serde_json::Value, Option<serde_json::Value>) {
    if let Some(root) = configured {
        return (
            json!({ "path": root.to_string_lossy(), "source": "config" }),
            None,
        );
    }
    match resolve_mumu_manager(None) {
        Ok(resolved) => (
            json!({
                "path": resolved.install_root.to_string_lossy(),
                "source": mumu_root_source(resolved.source),
            }),
            None,
        ),
        Err(error) => {
            // The code order of a provider startup discovery refusal.
            let reason = error
                .nemu_resolution_context()
                .map(|context| context.reason().as_str().to_owned())
                .or_else(|| {
                    error.diagnostic().map(|diagnostic| {
                        format!("{}.{}", diagnostic.category().as_str(), diagnostic.stage())
                    })
                })
                .unwrap_or_else(|| "device_error".to_owned());
            (
                serde_json::Value::Null,
                Some(json!({ "reason": reason, "message": error.message() })),
            )
        }
    }
}

/// `ExplicitRoot` is the configured root; the resolver rungs reuse the install source words.
fn mumu_root_source(source: MumuManagerSource) -> &'static str {
    match source {
        MumuManagerSource::ExplicitRoot => "config",
        MumuManagerSource::FolderEnvironment => "env",
        MumuManagerSource::RunningProcess => MumuInstallSource::RunningProcess.as_str(),
        MumuManagerSource::RegistryUninstall => MumuInstallSource::RegistryUninstall.as_str(),
        MumuManagerSource::VendorEnumeration => MumuInstallSource::VendorEnumeration.as_str(),
    }
}
