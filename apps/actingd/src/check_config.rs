// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::outcome::Record;
use actingcommand_contract::{
    ConfigParameter, FactScalar, InstanceResourcePackage, RuntimeConfigManifest,
};
use actingcommand_device::{MumuInstallSource, MumuManagerSource, resolve_mumu_manager};
use actingcommand_runtime_host::{
    CatalogGeneration, CatalogPreview, CatalogPreviewRequest, ExecutionBackendProvider,
    ExecutionBackendRegistry, ResolvedAdbEndpoint, ResolvedInstanceEndpoint, RuntimeHostConfig,
    inspect_standard_packs, preview_policy_catalog_transition,
};
use actingcommand_vision_ffi::{InvalidModelFolder, OnnxExecutionProvider, VisionListingCode};
use config::InstanceBindingKey;
use serde_json::json;

/// Workflow #378 A6a (model section 7.4): check-config v2, `{schema_version, outcome, report?}`.
pub(super) const SCHEMA_VERSION: &str = "actingcommand.actingd.check-config.v2";
/// Inputs this command cannot validate: vision model content is read and hashed only when a
/// model is first used (Workflow #360). Since Workflow #361 C3 the state root is read
/// read-only; `state_root_absent` is added when it does not exist. Workflow #387 D1b: the
/// instruction values a running Runtime holds are not read (only the files are); a running
/// Runtime reports them in `config.parameters.<section>` (`actingctl status --config`).
const NOT_CHECKED: [&str; 2] = ["vision_model_content", "instruction_values"];
/// Added to `not_checked` when the configured state root does not exist (Workflow #361 C3).
const STATE_ROOT_ABSENT_NOT_CHECKED: &str = "state_root_absent";
/// Added to `not_checked` when no MuMu install root could be resolved.
const MUMU_DISCOVERY_NOT_CHECKED: &str = "mumu_discovery";
/// The `device_paths` names the manifest reports (Workflow #318, cfg2), echoed in order.
const DEVICE_PATH_NAMES: [&str; 5] = [
    "nemu_folder",
    "nemu_ipc_dll",
    "droidcast_apk",
    "minitouch_path",
    "maatouch_path",
];

/// Loads, assembles and validates a configuration exactly as startup would, then previews
/// startup's catalog plan against the state root's ledger, read-only (Workflow #361 C3): the
/// database is never written, no lock is taken (no owner lock) and nothing is staged, and no
/// socket is bound. SQLite may leave the `-wal` / `-shm` sidecars of a cleanly stopped root.
///
/// Workflow #378 A6a (model section 7.4): a configuration that passes is `check_config_passed`
/// with the report (`report`); what the check noted (each ignored environment override, an
/// unresolved MuMu installation, an unused device binding, a broken standard-pack root, a
/// missing fact reader) is a `note` link of it. A refused one is the refusal's code with the
/// `stage` that refused it and the values that name what was refused.
pub(super) fn run(arguments: Vec<std::ffi::OsString>) -> Result<Reported, ActingdError> {
    if arguments.len() > 3 {
        return Err(ActingdError::config(ActingdCode::CheckConfigUsageInvalid));
    }
    let mut config = None;
    let (pairs, remaining) = arguments[1..].as_chunks::<2>();
    for pair in pairs {
        if pair[0] != "--config" || config.is_some() || pair[1].is_empty() {
            return Err(ActingdError::config(ActingdCode::CheckConfigOptionInvalid));
        }
        config = Some(PathBuf::from(&pair[1]));
    }
    if !remaining.is_empty() {
        return Err(ActingdError::config(ActingdCode::CheckConfigOptionInvalid));
    }
    let config_path =
        config.ok_or_else(|| ActingdError::config(ActingdCode::CheckConfigConfigMissing))?;
    let file = config::load_for(
        &config_path,
        actingcommand_contract::InstallConfigPurpose::CandidateCheck,
    )
    .map_err(|code| staged(ActingdError::config(code), ActingdLocation::ConfigLoad))?;
    // Workflow #398 RD-1b: the device refusal's outcome, when the assembly meets it.
    let device = file.device_refusal();
    // Workflow #369 S6a: the instance a `start_emulator` refusal names.
    let start_emulator = file.start_emulator_refusal();
    let RuntimeAssembly {
        host,
        provider,
        policy,
        manifest,
        config_selection,
        resource_packages,
        recognition_packages,
        ignored_env_overrides,
        adb_requirement,
        unbound_handsets,
        unused_device_bindings,
        emulator_autostart,
    } = file.assemble_or_refusal().map_err(|(code, refusal)| {
        let error = match device.filter(|device| device.code() == code) {
            Some(device) => {
                ActingdError::refused(code, device.outcome()).with_detail(device.to_string())
            }
            None => start_emulator_refused(code, start_emulator)
                .unwrap_or_else(|| ActingdError::refused(code, refusal)),
        };
        staged(error, ActingdLocation::ConfigAssemble)
    })?;
    // Workflow #398 RD-1b (model-398 v1.1 §3.6): a handset the private binding file binds
    // no device to is refused here; a start registers it unavailable and goes on.
    if let Some(alias) = unbound_handsets.into_iter().next() {
        let refused = config::DeviceRefusal::Missing(alias);
        return Err(staged(
            ActingdError::refused(refused.code(), refused.outcome())
                .with_detail(refused.to_string()),
            ActingdLocation::ConfigAssemble,
        ));
    }
    // Workflow #337: the install root's adb, checked exactly as at startup.
    let adb_default = config::ac_adb::require(&adb_requirement).map_err(|refused| {
        staged(
            ActingdError::refused(refused.code(), refused.outcome())
                .with_detail(refused.to_string()),
            ActingdLocation::ConfigAssemble,
        )
    })?;
    // Workflow #360: the vision root listed exactly as startup lists it; no model file is
    // read. A folder that breaks the folder rule fails this check, though startup would
    // only record it.
    let vision = match provider.vision() {
        None => None,
        Some(vision) => {
            let inventory = vision.inspect().map_err(|refused| {
                let outcome = Outcome::new(refused.code);
                let outcome = match refused.cause.clone() {
                    Some(cause) => outcome.caused_by(cause),
                    None => outcome,
                };
                staged(
                    ActingdError::refused(refused.code, outcome)
                        .with_detail(refused.message.clone()),
                    ActingdLocation::ConfigVision,
                )
            })?;
            if !inventory.listing.invalid.is_empty() {
                return Err(staged(
                    vision_folders_refused(&inventory.listing.invalid),
                    ActingdLocation::ConfigVision,
                ));
            }
            Some(vision_report(vision, &inventory))
        }
    };
    let modes = provider.modes();
    let deferred = provider.deferred_bindings();
    let mumu_root = provider.mumu_root().map(Path::to_path_buf);
    let env_nemu_folder = provider.env_nemu_folder().map(Path::to_path_buf);
    // Registered exactly as startup registers, short of discovery: the registry's own
    // refusals (duplicate aliases or instance ids) carry the code startup would report.
    let registry = provider.into_registry().map_err(|error| {
        staged(
            ActingdError::runtime(error),
            ActingdLocation::ConfigAssemble,
        )
    })?;
    host.validate().map_err(|error| {
        staged(
            ActingdError::runtime(error),
            ActingdLocation::ConfigValidate,
        )
    })?;
    let resource_packages =
        config::validate_resource_packages(&resource_packages).map_err(|refused| {
            staged(
                ActingdError::refused(refused.code, refused.outcome())
                    .with_detail(refused.to_string()),
                ActingdLocation::ResourcePackage,
            )
        })?;
    // Workflow #384 D2-S2 (D2 model §3.2): each recognition package binding is checked in
    // full (checks 1-2; check 3 is gone since Workflow #390 POOL-1) and a binding that is no
    // reader is refused, so `acsetup --commit-config` does not commit it. The realistic facts
    // the catalog reads that no reader marks are noted.
    let recognition = config::recognition_package::check_bindings(
        &recognition_packages,
        &host,
        config::recognition_package::BindingRead::Prepare,
    );
    if let Some(invalid) = recognition
        .iter()
        .find_map(|binding| binding.verdict.as_ref().err())
    {
        return Err(staged(
            ActingdError::refused(
                actingcommand_runtime_host::HostCode::FactReaderBindingInvalid,
                invalid.clone(),
            ),
            ActingdLocation::RecognitionBinding,
        ));
    }
    let missing_readers = config::recognition_package::missing_reader_warnings(
        policy.as_ref(),
        &config::recognition_package::marked_keys(&recognition),
    )
    .map_err(|code| {
        staged(
            ActingdError::config(code),
            ActingdLocation::RecognitionBinding,
        )
    })?;
    let checked = CheckedAssembly {
        host,
        registry,
        modes,
        deferred,
        mumu_root,
        env_nemu_folder,
        policy_configured: policy.is_some(),
        manifest,
        config_selection,
        ignored_env_overrides,
        adb_default,
        vision,
        unused_device_bindings,
        emulator_autostart,
    };
    let (mut report, mut notes) = summarize(&config_path, &checked, &resource_packages)
        .map_err(|(code, stage)| staged(ActingdError::config(code), stage))?;
    // Workflow #384 D2-S2: the bound recognition package of each instance that names one, and
    // the readers missing for the catalog's realistic facts.
    if let Some(instances) = report["instances"].as_array_mut() {
        for entry in instances {
            if let Some(binding) = recognition
                .iter()
                .find(|binding| entry["alias"].as_str() == Some(binding.alias.as_str()))
            {
                entry["recognition_package"] = json!({
                    "package": binding.path.to_string_lossy(),
                    "fact_keys": binding.verdict.as_ref().ok(),
                });
            }
        }
    }
    notes.extend(missing_readers);
    // Workflow #386 SR-3 (scheduling model §2.4 item 2): with
    // `scheduling.pack_admission.check_in_check_config` on, this Runtime's kernel admits every
    // package `policy.procedure_manifest` locates; one refused with a suspend code is a
    // `pack_admission_suspended` note and the configuration stays valid. Nothing is written.
    let pack_admission =
        actingcommand_runtime_config::EffectiveConfig::from_manifest(&checked.manifest)
            .and_then(|config| config.pack_admission())
            .map_err(|failure| {
                staged(
                    ActingdError::refused(failure.failure_code(), failure.outcome()),
                    ActingdLocation::ConfigAssemble,
                )
            })?;
    if pack_admission.check_in_check_config {
        notes.extend(actingcommand_runtime_host::pack_admission_notes(
            &checked.host,
            &pack_admission.suspend_codes,
            option_env!("ACTINGCOMMAND_RUNTIME_HEAD"),
        ));
    }
    // Workflow #361 C3: the plan startup would make, from the ledger read-only.
    let previewing = std::time::Instant::now();
    let preview = preview_policy_catalog_transition(&CatalogPreviewRequest {
        state_root: checked.host.state_root(),
        catalog: policy.as_ref().map(|policy| &policy.catalog),
        transition: policy
            .as_ref()
            .and_then(|policy| policy.catalog_transition.as_ref()),
        approval_ids: policy
            .as_ref()
            .map_or(&[][..], |policy| policy.catalog_approval_ids.as_slice()),
    })
    .map_err(|refused| {
        // Review P4: the time the preview ran before it failed, on the developer line.
        let elapsed_ms = u64::try_from(previewing.elapsed().as_millis()).unwrap_or(u64::MAX);
        staged(
            ActingdError::runtime(refused).with_detail(format!("elapsed_ms={elapsed_ms}")),
            ActingdLocation::PolicyState,
        )
    })?;
    report["policy_plan"] = policy_plan_report(&preview);
    if !preview.state_root_present()
        && let Some(not_checked) = report["not_checked"].as_array_mut()
    {
        not_checked.push(json!(STATE_ROOT_ABSENT_NOT_CHECKED));
    }
    let passed = notes.into_iter().fold(
        Outcome::new(ActingdCode::CheckConfigPassed),
        |outcome, note| outcome.joined(CauseRelation::Note, note),
    );
    Ok(Reported::new(passed).with("report", report))
}

/// The error with the check-config stage that refused (Workflow #378 A6a: its outcome's
/// `stage`).
fn staged(mut error: ActingdError, stage: ActingdLocation) -> ActingdError {
    error.stage = Some(stage.location());
    error
}

/// A check-config refusal of the summary: its code and the stage that refused.
type Refusal = (Code, ActingdLocation);

/// A refusal with a registered code.
fn registered(code: impl Into<Code>, stage: ActingdLocation) -> Refusal {
    (code.into(), stage)
}

/// Workflow #360: a vision root whose model folders break the folder rule, as
/// `vision_model_folder_invalid` with each folder in `invalid_models` (Workflow #378 A6a); each
/// folder's reason is developer text until B4 codes it.
fn vision_folders_refused(folders: &[InvalidModelFolder]) -> ActingdError {
    let code = VisionListingCode::VisionModelFolderInvalid;
    let records = folders.iter().map(|folder| {
        Record::new()
            .model_name(&folder.name)
            .path(&folder.path)
            .failure_code(code)
    });
    let text = folders
        .iter()
        .map(|folder| {
            format!(
                "vision model folder '{}' is invalid: {}",
                folder.name, folder.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    ActingdError::refused(code, Outcome::new(code).invalid_models(records)).with_detail(text)
}

/// Everything `summarize` reports once every check passed: the registry as configured and
/// the declarations only startup can complete.
struct CheckedAssembly {
    host: RuntimeHostConfig,
    registry: ExecutionBackendRegistry,
    modes: BTreeMap<String, ScheduledExecutionMode>,
    deferred: BTreeMap<String, InstanceBindingKey>,
    mumu_root: Option<PathBuf>,
    /// The injected `ACTINGCOMMAND_NEMU_FOLDER` (`None` unless `allow_env_overrides`).
    env_nemu_folder: Option<PathBuf>,
    policy_configured: bool,
    manifest: RuntimeConfigManifest,
    /// Workflow #387 D1b: the configuration layers read, as startup records `config.selection`.
    config_selection: Option<actingcommand_contract::ConfigSelection>,
    /// Set `ACTINGCOMMAND_*` variables ignored because `allow_env_overrides` is off.
    ignored_env_overrides: Vec<&'static str>,
    /// Workflow #337: the install root's adb and its state; `None` outside an install root.
    adb_default: Option<config::ac_adb::AdbDefault>,
    /// Workflow #360: the listed vision root; `None` when `vision` is absent.
    vision: Option<serde_json::Value>,
    /// Workflow #398 RD-1b: the aliases the private binding file binds that no handset instance
    /// has, in file order.
    unused_device_bindings: Vec<String>,
    /// Workflow #369 S6a: the instances whose `start_emulator` is `true`.
    emulator_autostart: Vec<String>,
}

/// Workflow #361 C3: `policy_plan`, startup's catalog plan as previewed from the ledger.
fn policy_plan_report(preview: &CatalogPreview) -> serde_json::Value {
    let phases = preview.phases();
    let generation = |generation: Option<&CatalogGeneration>| {
        generation.map_or(serde_json::Value::Null, |generation| {
            json!({
                "catalog_id": generation.catalog_id(),
                "catalog_version": generation.catalog_version(),
                "catalog_hash": generation.catalog_hash(),
            })
        })
    };
    let ids = |records: &[actingcommand_contract::ApprovalDecisionRecord]| {
        records
            .iter()
            .map(|record| record.approval_id().to_owned())
            .collect::<Vec<_>>()
    };
    json!({
        "state": if preview.state_root_present() { "read" } else { "state_root_absent" },
        "active": generation(preview.active()),
        "driver": if preview.configured().is_some() { "on" } else { "off" },
        "configured": generation(preview.configured()),
        "plan": preview.plan().map(|plan| plan.as_str()),
        "approvals": preview.approvals().map(|approvals| json!({
            "record": ids(approvals.record()),
            "reapprove": ids(approvals.reapprove()),
            "revoke": ids(approvals.revoke()),
        })),
        // Review P4: the time of each phase, so a slow ledger read is measured, not guessed.
        "phase_ms": {
            "compile": phases.compile_ms,
            "ledger": phases.ledger_ms,
            "projection": phases.projection_ms,
            "approvals": phases.approvals_ms,
            "total": phases.total_ms,
        },
    })
}

/// The vision root as listed: its path, execution provider and every model folder.
fn vision_report(
    vision: &config::ConfiguredVision,
    inventory: &config::VisionInventory,
) -> serde_json::Value {
    json!({
        "root": inventory.root.to_string_lossy(),
        "execution_provider": match vision.execution_provider() {
            OnnxExecutionProvider::Cpu => "cpu",
            OnnxExecutionProvider::Cuda => "cuda",
        },
        "onnxruntime_library": inventory.runtime.onnxruntime_library.to_string_lossy(),
        "ocr_models": inventory
            .listing
            .ocr
            .iter()
            .map(|spec| json!({
                "model_ref": spec.model_ref,
                "layout": spec.layout.as_str(),
                "detector": spec.detector_path.is_some(),
                "description_sha256": spec.description_sha256,
                "languages": spec.description.languages,
            }))
            .collect::<Vec<_>>(),
        "nn_models": inventory
            .listing
            .nn
            .iter()
            .map(|spec| json!({
                "model_ref": spec.model_ref,
                "description_sha256": spec.description_sha256,
                "languages": spec.description.languages,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The report of a configuration that passed, and what the check noted beside it (Workflow
/// #378 A6a: the `note` links of `check_config_passed`, which were v1's `warnings` and
/// `mumu_root_unresolved`).
fn summarize(
    config_path: &Path,
    checked: &CheckedAssembly,
    resource_packages: &BTreeMap<String, InstanceResourcePackage>,
) -> Result<(serde_json::Value, Vec<Outcome>), Refusal> {
    let registry = &checked.registry;
    let mut instances = registry
        .instance_aliases()
        .into_iter()
        .map(|alias| {
            let mode = match checked.modes.get(&alias) {
                Some(ScheduledExecutionMode::DeviceRegistry) => "device_registry",
                Some(ScheduledExecutionMode::FixtureSimulation) => "fixture_simulation",
                None => {
                    return Err(registered(
                        SharedCode::ExecutionBackendRegistryIncomplete,
                        ActingdLocation::ConfigValidate,
                    ));
                }
            };
            // The startup package as assembled: its locator, neither opened nor hashed
            // (Workflow #336 PKG-S4: no digest).
            let startup_package = checked
                .host
                .startup_packages()
                .get(&alias)
                .map(|request| json!({ "package": request.package_path() }));
            // The effective stuck-recovery ladder settings (slice #316-B4): the instance's own,
            // or the `recovery_ladder` section's (Workflow #387 D1c).
            let stuck_recovery = checked
                .host
                .stuck_recovery()
                .get(&alias)
                .copied()
                .ok_or_else(|| {
                    registered(
                        SharedCode::ExecutionBackendRegistryIncomplete,
                        ActingdLocation::ConfigValidate,
                    )
                })?;
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
                    // Workflow #369 S6a: started by the daemon when stopped at its start (S6c).
                    "start_emulator": checked.emulator_autostart.contains(&alias),
                }));
            }
            let resolved = registry.resolve(&alias).ok_or_else(|| {
                registered(
                    SharedCode::ExecutionBackendRegistryIncomplete,
                    ActingdLocation::ConfigValidate,
                )
            })?;
            // Workflow #398 RD-1b: a handset is addressed by its serial and has no host or port.
            let endpoint = resolved
                .adb_endpoint()
                .and_then(ResolvedInstanceEndpoint::bound)
                .filter(|endpoint| endpoint.handset().is_none());
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
    // Workflow #336 PKG-S1: every `resource_package` directory is admitted in full by its path,
    // so none is left unchecked.
    let mut not_checked = NOT_CHECKED.to_vec();
    // Workflow #318 (cfg3): one `env_override_ignored` per set variable that
    // `allow_env_overrides` off leaves unread.
    let mut notes = config::env_override_warnings(&checked.ignored_env_overrides);
    let (mumu_root, mumu_root_unresolved) = mumu_root_report(
        checked.mumu_root.as_deref(),
        checked.env_nemu_folder.as_deref(),
    );
    if let Some(unresolved) = mumu_root_unresolved {
        not_checked.push(MUMU_DISCOVERY_NOT_CHECKED);
        notes.push(unresolved);
    }
    // Workflow #398 RD-1b: each alias the private binding file binds that no handset has.
    notes.extend(
        checked
            .unused_device_bindings
            .iter()
            .map(|alias| Outcome::new(ActingdCode::DeviceBindingUnused).alias(alias)),
    );
    let bind_address = checked.host.bind_address();
    // The new tunables (Workflow #318, cfg2) echoed from the manifest, so this report and
    // `actingctl status --config` cannot disagree; since Workflow #385 P-c rows of the
    // `performance` section.
    let performance = json!({
        "pressure_start_samples": manifest_integer(
            &checked.manifest,
            actingcommand_runtime_config::performance::monitor::PRESSURE_START_SAMPLES,
        )?,
        "pressure_end_samples": manifest_integer(
            &checked.manifest,
            actingcommand_runtime_config::performance::monitor::PRESSURE_END_SAMPLES,
        )?,
    });
    let mut device_paths = serde_json::Map::new();
    for name in DEVICE_PATH_NAMES {
        let value = match manifest_parameter(&checked.manifest, &format!("device_paths.{name}")) {
            Some(ConfigParameter {
                value: FactScalar::String(path),
                source,
                ..
            }) => json!({ "path": path, "source": source.as_str() }),
            Some(_) => {
                return Err(registered(
                    ActingdCode::ConfigManifestIncomplete,
                    ActingdLocation::ConfigAssemble,
                ));
            }
            None => serde_json::Value::Null,
        };
        device_paths.insert(name.to_owned(), value);
    }
    let mut report = json!({
        "config_path": config_path.to_string_lossy(),
        "state_root": checked.host.state_root().to_string_lossy(),
        "bind_host": bind_address.ip().to_string(),
        "bind_port": bind_address.port(),
        "instance_count": instances.len(),
        "instances": instances,
        "policy_configured": checked.policy_configured,
        "performance": performance,
        "device_paths": device_paths,
        "config_manifest": checked.manifest,
        // Workflow #387 D1b: what startup records as `config.selection`; the build compares
        // `defaults_sha256` with the shipped defaults file.
        "config_selection": checked.config_selection,
        "not_checked": not_checked,
        "mumu_root": mumu_root,
        // Workflow #337: `null` outside an install root, else `{ path, state }`.
        "adb_default": checked
            .adb_default
            .as_ref()
            .map(config::ac_adb::AdbDefault::report),
        // Workflow #360: `null` when `vision` is absent, else the listed vision root.
        "vision": checked.vision,
    });
    // Workflow #373 NAV-6 (M-373 §7): the standard-pack root as startup reads it (root files
    // only, no package admitted). A broken game root is that game's problem and one note, its
    // problem's outcome; the configuration itself is not refused.
    let standard_packs = inspect_standard_packs(checked.host.standard_packs_root());
    notes.extend(standard_packs.root_problem.iter().cloned());
    notes.extend(
        standard_packs
            .problems()
            .map(|(_, problem)| problem.clone()),
    );
    report["standard_packs"] = serde_json::to_value(&standard_packs).map_err(|_| {
        registered(
            ActingdCode::CheckConfigReportEncodeFailed,
            ActingdLocation::ConfigAssemble,
        )
    })?;
    Ok((report, notes))
}

/// One manifest parameter by key; `None` when the manifest does not carry it.
fn manifest_parameter<'a>(
    manifest: &'a RuntimeConfigManifest,
    key: &str,
) -> Option<&'a ConfigParameter> {
    manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == key)
}

/// `{ "value": <integer>, "source": <source> }` of a parameter the manifest must carry as
/// an integer; anything else is an incomplete manifest, refused rather than filled in.
fn manifest_integer(
    manifest: &RuntimeConfigManifest,
    key: &str,
) -> Result<serde_json::Value, Refusal> {
    match manifest_parameter(manifest, key) {
        Some(ConfigParameter {
            value: FactScalar::Integer(value),
            source,
            ..
        }) => Ok(json!({ "value": value, "source": source.as_str() })),
        _ => Err(registered(
            ActingdCode::ConfigManifestIncomplete,
            ActingdLocation::ConfigAssemble,
        )),
    }
}

/// The MuMu install root the daemon would use: the configured `mumu_root`, else one
/// read-only `resolve_mumu_manager(None, env_folder)` run (the injected
/// `ACTINGCOMMAND_NEMU_FOLDER` only under `allow_env_overrides`, running process, uninstall
/// registry, vendor folders). `MuMuManager.exe` is never run; no instance is started or
/// stopped. A failed resolution is reported as `null` plus its outcome, a note of the passed
/// check, never refused: the Nemu resolution reason, else `provider_native_failed` with the
/// device failure's category, as provider startup records it (Workflow #378 A6a: was v1's
/// `mumu_root_unresolved`).
fn mumu_root_report(
    configured: Option<&Path>,
    env_folder: Option<&Path>,
) -> (serde_json::Value, Option<Outcome>) {
    if let Some(root) = configured {
        return (
            json!({ "path": root.to_string_lossy(), "source": "config" }),
            None,
        );
    }
    match resolve_mumu_manager(None, env_folder) {
        Ok(resolved) => (
            json!({
                "path": resolved.install_root.to_string_lossy(),
                "source": mumu_root_source(resolved.source),
            }),
            None,
        ),
        Err(error) => (
            serde_json::Value::Null,
            Some(config::device_failure_outcome(&error, None)),
        ),
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
