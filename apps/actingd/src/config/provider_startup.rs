// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::outcome::Record;
use actingcommand_contract::{
    DiscoveredInstanceObservation, LifecycleNativeDetail, ProviderBackend, ProviderNativeFailure,
    ProviderStartupObservation as Observation, ProviderStartupStage as Stage,
};
use actingcommand_device::{
    DeviceError, DiscoveredMumuInstance, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityProfile, MumuDiscoveryReport, MumuEmulatorCapabilityBackend,
    NemuResolutionContext,
};
use actingcommand_execution_kernel::InstanceDiscoveryFailure;
use actingcommand_ppocr_onnx_json_provider::PpocrCtcLoader;
use actingcommand_runtime_host::{
    DiscoveredInstanceBinding, ProviderStartup, RuntimeHostResult, admit_emulator_capabilities,
};
use actingcommand_vision_ffi::{ONNXRUNTIME_LIBRARY, VISION_RUNTIME_DIRECTORY, VisionListingCode};

impl ConfiguredProvider {
    /// Provider startup: completes the discovery-bound instances, constructs the vision
    /// provider and hands everything to the registry once. Every refusal is recorded as a
    /// Provider startup failure before startup fails.
    pub(crate) fn assemble_provider(
        self,
        startup: &mut ProviderStartup<'_>,
    ) -> RuntimeHostResult<Arc<dyn ExecutionBackendProvider>> {
        let discovery = self.discovery_spec();
        let Self {
            mut instances,
            deferred,
            vision,
            env_overrides,
            ..
        } = self;
        if !deferred.is_empty() {
            instances.extend(bind_discovered_instances(
                startup,
                discovery.mumu_root(),
                || discovery.discover(),
                &mut std::io::stderr(),
                deferred,
            )?);
        }
        // The injected diagnostic value is carried verbatim; the provider validates it (a
        // non-UTF-8 value becomes a lossy string the provider refuses).
        let node_placement_diagnostic = env_overrides
            .ppocr_node_placement_diagnostic
            .map(|value| value.to_string_lossy().into_owned());
        let vision = match vision {
            None => {
                startup.record(ProviderBackend::Configured, Observation::NotConfigured)?;
                None
            }
            Some(vision) => Some(VisionSpec::new(assemble_vision_provider(
                startup,
                &vision,
                node_placement_diagnostic,
            )?)),
        };
        let vision_configured = vision.is_some();
        let registry = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
            instances,
            vision,
            discovery: Some(discovery),
        })
        .map_err(|error| {
            startup.failed(
                ProviderBackend::Configured,
                Stage::RegistryBinding,
                error.code(),
                ProviderNativeFailure {
                    module: "actingcommand_runtime_host::provider".into(),
                    outcome: Outcome::relayed(error.code()),
                },
            )
        })?;
        if vision_configured {
            startup.record(ProviderBackend::Configured, Observation::Ready)?;
        }
        Ok(Arc::new(registry))
    }
}

/// Runs `MuMuManager` discovery once per startup (`discover`, tried again while it is
/// unavailable: `discover_at_start`, whose warnings also go to `daemon_log`, actingd's stderr)
/// and completes every deferred instance into its registration; the registry registers them
/// together with the explicit ones. Every refusal is recorded as a Provider startup failure
/// before startup fails.
pub(super) fn bind_discovered_instances(
    startup: &mut ProviderStartup<'_>,
    mumu_root: Option<&Path>,
    discover: impl FnMut() -> Result<MumuDiscoveryReport, Box<InstanceDiscoveryFailure>>,
    daemon_log: &mut dyn std::io::Write,
    deferred: Vec<DeferredInstance>,
) -> RuntimeHostResult<Vec<InstanceSpec>> {
    let backend = ProviderBackend::MumuManager;
    let stage = Stage::InstanceDiscovery;
    startup.record(backend, Observation::Started { stage })?;
    let report = discover_at_start(startup, mumu_root, discover, daemon_log)?;
    let profile = admit_capability_profile(startup, &report, mumu_root)?;
    let mut bound = BTreeMap::new();
    let mut resolved = Vec::new();
    let mut refusal = None;
    for entry in deferred {
        let alias = entry.alias.clone();
        match resolve_deferred_instance(entry, &report, &profile) {
            Ok((instance_index, spec)) => {
                bound.insert(instance_index, alias);
                resolved.push(spec);
            }
            Err(failure) => {
                refusal = Some(failure);
                break;
            }
        }
    }
    // `run_json` already refused a non-UTF-8 MuMuManager path, so this is lossless here.
    startup.record(
        backend,
        Observation::InstanceDiscovery {
            source: report.source.as_str().to_owned(),
            mumu_manager_path: report.mumu_manager_path.to_string_lossy().into_owned(),
            version: report.version.to_string(),
            instances: report
                .instances
                .iter()
                .map(|instance| DiscoveredInstanceObservation {
                    instance_index: instance.instance_index,
                    instance_name: instance.instance_name.clone(),
                    adb_host: instance.adb_host.clone(),
                    adb_port: instance.adb_port,
                    running: instance.running,
                    bound_alias: bound.get(&instance.instance_index).cloned(),
                })
                .collect(),
        },
    )?;
    if let Some((classification, failure)) = refusal {
        return Err(startup.failed(backend, stage, classification.as_str(), failure));
    }
    startup.record(backend, Observation::Completed { stage })?;
    Ok(resolved)
}

/// Workflow #390 DISC-1 (coordinator ruling 2 of wave 18): the startup discovery, tried up to
/// `startup_discovery.attempts` times while it answers `instance_discovery_unavailable` (as after
/// a boot where the Runtime starts before the emulator), `startup_discovery.retry_interval_ms`
/// apart on the Runtime clock. Each failed attempt but the last is recorded as the warning
/// `instance_discovery_retried`, with the attempt's failure as its `caused_by` link, before the
/// wait, and written to `daemon_log` as actingd's daemon lines of that warning (its developer
/// line, then its `OUTCOME actingd:` line), so a person or a start script reading stderr sees
/// the start wait. Any other refusal, or the last attempt's, is recorded as the Provider startup
/// failure and ends the start as before.
fn discover_at_start(
    startup: &mut ProviderStartup<'_>,
    mumu_root: Option<&Path>,
    mut discover: impl FnMut() -> Result<MumuDiscoveryReport, Box<InstanceDiscoveryFailure>>,
    daemon_log: &mut dyn std::io::Write,
) -> RuntimeHostResult<MumuDiscoveryReport> {
    let backend = ProviderBackend::MumuManager;
    let stage = Stage::InstanceDiscovery;
    let retry = startup.startup_discovery();
    let mut attempt = 1_u32;
    loop {
        let InstanceDiscoveryFailure {
            code: classification,
            error,
        } = match discover() {
            Ok(report) => return Ok(report),
            Err(failure) => *failure,
        };
        let failure = device_failure_outcome(&error, mumu_root);
        // RED PROOF mutation 1 (the 10-16 defect): one attempt only, then the fatal.
        if classification != SharedCode::InstanceDiscoveryUnavailable.as_str()
            || attempt >= retry.attempts.min(1)
        {
            return Err(startup.failed(
                backend,
                stage,
                classification,
                ProviderNativeFailure {
                    module: "actingcommand_device::mumu_manager".into(),
                    outcome: failure,
                },
            ));
        }
        let warning = Outcome::new(ActingdCode::InstanceDiscoveryRetried)
            .attempts(attempt)
            .limit_count(retry.attempts)
            .retry_after_ms(retry.retry_interval_ms)
            .caused_by(Outcome::relayed(classification).caused_by(failure));
        startup.record(
            backend,
            Observation::DiscoveryRetried {
                outcome: warning.clone(),
            },
        )?;
        // A failed write never stops the start (P3 L2): stderr is the last place actingd
        // reports to.
        let _unwritten = crate::write_daemon_reports(std::slice::from_ref(&warning), daemon_log);
        startup.pause(Duration::from_millis(retry.retry_interval_ms))?;
        attempt += 1;
    }
}

/// The outcome of a device-layer discovery failure (ledger revision 3): a Nemu resolution's
/// registered reason with the context its resolving step recorded (Workflow #378 A7), else
/// `provider_native_failed` with the device failure's category, with the configured
/// `mumu_root`; the device's own text is its `foreign_other_error` link. The Runtime's summary
/// of the failure (`diagnostic_message`) and the stage are not persisted.
pub(crate) fn device_failure_outcome(error: &DeviceError, mumu_root: Option<&Path>) -> Outcome {
    let outcome = match (error.nemu_resolution_context(), error.diagnostic()) {
        (Some(context), _) => nemu_resolution_outcome(context),
        (None, Some(diagnostic)) => {
            Outcome::new(ActingdCode::ProviderNativeFailed).device_category(diagnostic.category())
        }
        (None, None) => Outcome::new(ActingdCode::ProviderNativeFailed),
    };
    let outcome = match mumu_root {
        Some(root) => outcome.mumu_root(root),
        None => outcome,
    };
    match LifecycleNativeDetail::bounded(error.message()) {
        Some(native) => outcome.caused_by(native.outcome()),
        None => outcome,
    }
}

/// Workflow #378 A7 (A6a left the MuMu note's details to A7): a Nemu resolution reason with the
/// context its resolving step recorded as values, each only when the step recorded it: what it
/// counted (`count_kind`, `count`, and `count_known`, false for a lower bound), the resolver
/// rung that chose the installation (`install_source`), and how the adb, the MuMu folder and the
/// capture DLL were configured (`configured_adb_class`, `explicit_root`, `explicit_dll`).
fn nemu_resolution_outcome(context: NemuResolutionContext) -> Outcome {
    let mut outcome = Outcome::new(context.reason());
    if let Some(kind) = context.count_kind() {
        outcome = outcome.count_kind(kind);
    }
    if let Some(count) = context.count() {
        outcome = outcome.count(count);
    }
    if let Some(known) = context.count_known() {
        outcome = outcome.count_known(known);
    }
    if let Some(source) = context.source() {
        outcome = outcome.install_source(source);
    }
    if let Some(class) = context.configured_adb() {
        outcome = outcome.configured_adb_class(class);
    }
    if let Some(explicit) = context.explicit_root() {
        outcome = outcome.explicit_root(explicit);
    }
    if let Some(explicit) = context.explicit_dll() {
        outcome = outcome.explicit_dll(explicit);
    }
    outcome
}

/// Admits the capability profile derived from the discovery report (pure, nothing is
/// dispatched) and records it as one `capability_profile` observation inside a
/// `capability_admission` bracket. A refusal is recorded before startup fails, with the
/// refused MuMuManager version and the configured `mumu_root`, and the host's refusal as its
/// `caused_by` link (ledger revision 3).
fn admit_capability_profile(
    startup: &mut ProviderStartup<'_>,
    report: &MumuDiscoveryReport,
    mumu_root: Option<&Path>,
) -> RuntimeHostResult<EmulatorCapabilityProfile> {
    let backend = ProviderBackend::MumuManager;
    let stage = Stage::CapabilityAdmission;
    startup.record(backend, Observation::Started { stage })?;
    let mut capability_backend = MumuEmulatorCapabilityBackend::new(report.clone());
    let profile = admit_emulator_capabilities(
        &mut capability_backend,
        &[
            EmulatorCapability::InventoryRead.as_str(),
            EmulatorCapability::InstanceStatusRead.as_str(),
        ],
    )
    .map_err(|error| {
        let refused = Outcome::new(ActingdCode::EmulatorCapabilityAdmissionRefused)
            .mumu_manager_version(report.version.to_string());
        let refused = match mumu_root {
            Some(root) => refused.mumu_root(root),
            None => refused,
        };
        startup.failed(
            backend,
            stage,
            ActingdCode::EmulatorCapabilityAdmissionRefused.as_str(),
            ProviderNativeFailure {
                module: "actingcommand_runtime_host::emulator_control".into(),
                outcome: refused.caused_by(Outcome::relayed(error.code())),
            },
        )
    })?;
    let ids = |availability| {
        profile
            .capability_ids_with(availability)
            .into_iter()
            .map(str::to_owned)
            .collect()
    };
    startup.record(
        backend,
        Observation::CapabilityProfile {
            provider_id: profile.provider_id().to_owned(),
            // The pure builder records exactly this discovered version as the profile version.
            version: report.version.to_string(),
            available: ids(EmulatorCapabilityAvailability::Available),
            unverified: ids(EmulatorCapabilityAvailability::Unverified),
            unavailable: ids(EmulatorCapabilityAvailability::Unavailable),
        },
    )?;
    startup.record(backend, Observation::Completed { stage })?;
    Ok(profile)
}

/// A deferred instance's refusal: the startup failure code and the native failure it records.
type DeferredRefusal = (Code, ProviderNativeFailure);

/// Matches one deferred instance by index or exact name, cross-checks declared ADB values
/// against the discovered ones and builds its device registration carrying the admitted
/// provider capability profile.
fn resolve_deferred_instance(
    entry: DeferredInstance,
    report: &MumuDiscoveryReport,
    profile: &EmulatorCapabilityProfile,
) -> Result<(u16, InstanceSpec), DeferredRefusal> {
    let DeferredInstance {
        alias, key, config, ..
    } = entry;
    // Ledger revision 3: a refusal is its registered code with the instance's alias and the
    // declared value as values, and what discovery reported for the instance as `instances`
    // records (the candidates of an ambiguous name) or, for the adb path, `other_path`. The
    // whole discovery is the `instance_discovery` record written before it; no Runtime text is
    // kept.
    let failure = |outcome: Outcome| ProviderNativeFailure {
        module: "actingd.config".into(),
        outcome,
    };
    let refused = |code: ActingdCode| Outcome::new(code).alias(alias.as_str());
    let declared_key = |outcome: Outcome| match &key {
        InstanceBindingKey::Index(index) => outcome.instance_index(*index),
        InstanceBindingKey::Name(name) => outcome.instance_name(name.as_str()),
    };
    let discovered = |instance: &DiscoveredMumuInstance| {
        Record::new()
            .instance_index(instance.instance_index)
            .instance_name(instance.instance_name.as_str())
    };
    let matches = report
        .instances
        .iter()
        .filter(|instance| match &key {
            InstanceBindingKey::Index(index) => instance.instance_index == *index,
            InstanceBindingKey::Name(name) => instance.instance_name == *name,
        })
        .collect::<Vec<_>>();
    let instance = match matches.as_slice() {
        [] => {
            return Err((
                ActingdCode::InstanceDiscoveryNoMatch.code(),
                failure(declared_key(refused(ActingdCode::InstanceDiscoveryNoMatch))),
            ));
        }
        [instance] => *instance,
        candidates => {
            return Err((
                ActingdCode::InstanceDiscoveryAmbiguous.code(),
                failure(
                    declared_key(refused(ActingdCode::InstanceDiscoveryAmbiguous))
                        .instances(candidates.iter().copied().map(discovered)),
                ),
            ));
        }
    };
    // Workflow #337: an instance whose `adb_path` is absent or names the install root's adb
    // uses that adb, which `ac_adb::require` verified before the ledger opened; any other
    // declared `adb_path` must be the discovered MuMu adb.
    let installed_adb = config.adb.selected().map(|installed| {
        config
            .adb_path
            .clone()
            .unwrap_or_else(|| installed.path_string())
    });
    if installed_adb.is_none()
        && let Some(declared) = config.adb_path.as_deref()
        && !fs::canonicalize(declared).is_ok_and(|path| path == instance.adb_path)
    {
        return Err((
            ActingdCode::InstanceDiscoveryConflict.code(),
            failure(
                refused(ActingdCode::AdbPathConflict)
                    .adb_path(declared)
                    .other_path(&instance.adb_path)
                    .instances([discovered(instance)]),
            ),
        ));
    }
    if let (Some(declared), Some(adb_host)) = (config.host.as_deref(), instance.adb_host.as_deref())
        && declared.trim() != adb_host
    {
        return Err((
            ActingdCode::InstanceDiscoveryConflict.code(),
            failure(
                refused(ActingdCode::HostConflict)
                    .adb_host(declared.trim())
                    .instances([discovered(instance).adb_host(adb_host)]),
            ),
        ));
    }
    // A stopped instance reports no ADB endpoint (observed on MuMuManager 6.5.7.0): it is
    // bound PENDING, never with a guessed port, and `emulator start` resolves the port. A
    // declared port cannot be cross-checked then and is refused rather than trusted.
    match (config.port, instance.adb_port) {
        (Some(declared), Some(adb_port)) if declared != adb_port => {
            return Err((
                ActingdCode::InstanceDiscoveryConflict.code(),
                failure(
                    refused(ActingdCode::PortConflict)
                        .adb_port(declared)
                        .instances([discovered(instance).adb_port(adb_port)]),
                ),
            ));
        }
        // The instance is stopped, so a declared port cannot be cross-checked: omitting `port`
        // binds it pending.
        (Some(declared), None) => {
            return Err((
                ActingdCode::InstanceDiscoveryConflict.code(),
                failure(
                    refused(ActingdCode::PortUnverifiable)
                        .adb_port(declared)
                        .instances([discovered(instance).running(instance.running)]),
                ),
            ));
        }
        _ => {}
    }
    // The host a pending binding is completed with: the reported one, else the declared one,
    // else the daemon's explicit-entry default.
    let adb_host = instance
        .adb_host
        .clone()
        .or_else(|| config.host.as_deref().map(str::trim).map(str::to_owned))
        .unwrap_or_else(default_device_host);
    let adb_path = match installed_adb {
        Some(path) => path,
        None => instance
            .adb_path
            .to_str()
            .ok_or_else(|| {
                let refusal = refused(ActingdCode::AdbPathEncodingInvalid)
                    .other_path(&instance.adb_path)
                    .instances([discovered(instance)]);
                (
                    SharedCode::InstanceDiscoveryUnavailable.code(),
                    failure(match config.adb_path.as_deref() {
                        Some(declared) => refusal.adb_path(declared),
                        None => refusal,
                    }),
                )
            })?
            .to_owned(),
    };
    let binding = DiscoveredInstanceBinding::new(
        instance.instance_index,
        instance.instance_name.clone(),
        report.version.to_string(),
        instance.mumu_manager_path.clone(),
    );
    let adb_port = instance.adb_port;
    let registration = config
        .device_registration(adb_path, adb_host, adb_port)
        .map_err(|code| (code, failure(Outcome::new(code))))?;
    let registration = match adb_port {
        Some(_) => registration.with_discovered_binding(binding),
        None => registration.with_pending_discovered_binding(binding),
    }
    .with_capability_profile(profile.clone());
    Ok((instance.instance_index, InstanceSpec::real(registration)))
}

/// Workflow #360: lists the vision root and builds the provider over its model folders. Only
/// directory entries, file metadata and model descriptions are read; each model is read,
/// hashed and loaded, and ONNX Runtime initialised, on first use. A folder that breaks the
/// folder rule is recorded with its reason and fails only the requests that name it.
fn assemble_vision_provider(
    startup: &mut ProviderStartup<'_>,
    vision: &ConfiguredVision,
    node_placement_diagnostic: Option<String>,
) -> RuntimeHostResult<Arc<dyn RecognitionVisionProvider>> {
    let backend = ProviderBackend::Configured;
    let stage = Stage::PathBinding;
    startup.record(backend, Observation::Started { stage })?;
    let inventory = vision.inspect().map_err(|refusal| {
        startup.failed(
            backend,
            stage,
            refusal.code.as_str(),
            ProviderNativeFailure {
                module: "actingd.config".into(),
                outcome: match refusal.cause {
                    Some(cause) => Outcome::new(refusal.code).caused_by(cause),
                    None => Outcome::new(refusal.code),
                },
            },
        )
    })?;
    binding(
        startup,
        backend,
        "vision_root",
        vision.configured(),
        vision.base(),
        &inventory.root,
    )?;
    binding(
        startup,
        backend,
        "onnxruntime_library",
        &Path::new(VISION_RUNTIME_DIRECTORY).join(ONNXRUNTIME_LIBRARY),
        &inventory.root,
        &inventory.runtime.onnxruntime_library,
    )?;
    for (index, library) in inventory.runtime.runtime_library_closure.iter().enumerate() {
        binding(
            startup,
            backend,
            &format!("runtime_library_paths[{index}]"),
            library,
            &inventory.root,
            library,
        )?;
    }
    let models_dir = text(startup, backend, &inventory.models_dir)?;
    for spec in &inventory.listing.ocr {
        let folder = inventory.models_dir.join(&spec.model_ref);
        let folder = text(startup, ProviderBackend::FastdeployPpocr, &folder)?;
        startup.record(
            ProviderBackend::FastdeployPpocr,
            Observation::Binding {
                field: format!("ocr_model:{}", spec.model_ref),
                configured: format!(
                    "layout={}; family={}; detector={}; description_sha256={}; languages={:?}",
                    spec.layout.as_str(),
                    spec.description.family,
                    if spec.detector_path.is_some() {
                        "present"
                    } else {
                        "absent"
                    },
                    spec.description_sha256,
                    spec.description.languages
                ),
                base: models_dir.clone(),
                resolved: folder,
            },
        )?;
    }
    for spec in &inventory.listing.nn {
        let folder = inventory.models_dir.join(&spec.model_ref);
        let folder = text(startup, ProviderBackend::Onnxruntime, &folder)?;
        startup.record(
            ProviderBackend::Onnxruntime,
            Observation::Binding {
                field: format!("nn_model:{}", spec.model_ref),
                configured: format!(
                    "family={}; description_sha256={}; languages={:?}",
                    spec.description.family, spec.description_sha256, spec.description.languages
                ),
                base: models_dir.clone(),
                resolved: folder,
            },
        )?;
    }
    // Ledger revision 3: an invalid folder is its registered code with the folder's path (lossy
    // when it is not UTF-8) and its name when the name fits the key table's name rule (at most
    // 256 bytes, no control character); vision-ffi's reason stays in check-config's report.
    for folder in &inventory.listing.invalid {
        let outcome = Outcome::new(VisionListingCode::VisionModelFolderInvalid).path(&folder.path);
        let outcome = if folder.name.len() <= 256 && !folder.name.chars().any(char::is_control) {
            outcome.model_name(folder.name.as_str())
        } else {
            outcome
        };
        startup.record(backend, Observation::ModelFolderInvalid { outcome })?;
    }
    if inventory.listing.ocr.is_empty() {
        startup.record(ProviderBackend::FastdeployPpocr, Observation::NotConfigured)?;
    }
    if inventory.listing.nn.is_empty() {
        startup.record(ProviderBackend::Onnxruntime, Observation::NotConfigured)?;
    }
    startup.record(backend, Observation::Completed { stage })?;
    let stage = Stage::BackendConstruction;
    startup.record(backend, Observation::Started { stage })?;
    let loader = Arc::new(PpocrCtcLoader::new(
        inventory.runtime,
        node_placement_diagnostic,
    ));
    let provider = VisionFfiProvider::from_listing(inventory.listing, loader).map_err(|error| {
        startup.failed(
            backend,
            stage,
            ActingdCode::VisionProviderUnavailable.as_str(),
            // Ledger revision 3: the kernel error's code is the refusal's `caused_by` link
            // (relayed: the vision provider's codes are registered with B4); its text is the
            // Runtime's own and is not kept.
            ProviderNativeFailure {
                module: "execution-kernel".into(),
                outcome: Outcome::new(ActingdCode::VisionProviderUnavailable)
                    .caused_by(Outcome::relayed(error.code().as_str())),
            },
        )
    })?;
    startup.record(backend, Observation::Completed { stage })?;
    Ok(Arc::new(provider))
}

/// A path as recorded text; a path that is not UTF-8 is refused before it is recorded.
fn text(
    startup: &mut ProviderStartup<'_>,
    backend: ProviderBackend,
    value: &Path,
) -> RuntimeHostResult<String> {
    value.to_str().map(str::to_owned).ok_or_else(|| {
        startup.failed(
            backend,
            Stage::PathBinding,
            ActingdCode::VisionProviderPathEncodingInvalid.as_str(),
            ProviderNativeFailure {
                module: "actingd.config".into(),
                outcome: Outcome::new(ActingdCode::PathEncodingInvalid),
            },
        )
    })
}

fn binding(
    startup: &mut ProviderStartup<'_>,
    backend: ProviderBackend,
    field: &str,
    configured: &Path,
    base: &Path,
    resolved: &Path,
) -> RuntimeHostResult<()> {
    let configured = text(startup, backend, configured)?;
    let base = text(startup, backend, base)?;
    let resolved = text(startup, backend, resolved)?;
    startup.record(
        backend,
        Observation::Binding {
            field: field.into(),
            configured,
            base,
            resolved,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::IdentifierIssuer;
    use actingcommand_device::{MumuManagerSource, MumuManagerVersion, mumu_capability_profile};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// A discovery of four instances: one running, one stopped (no ADB endpoint) and two
    /// sharing a name.
    fn discovery_report() -> MumuDiscoveryReport {
        let version = MumuManagerVersion::parse("6.5.7.0").expect("a four-part version");
        let instance =
            |instance_index: u16, name: &str, port: Option<u16>| DiscoveredMumuInstance {
                install_root: PathBuf::from("C:/fixture"),
                mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
                adb_path: PathBuf::from("C:/fixture/nx_main/adb.exe"),
                instance_index,
                instance_name: name.to_owned(),
                adb_host: port.map(|_| "127.0.0.1".to_owned()),
                adb_port: port,
                running: port.is_some(),
                player_state: port.map(|_| "start_finished".to_owned()),
                android_version: Some("12".to_owned()),
                mumu_version: version,
            };
        MumuDiscoveryReport {
            source: MumuManagerSource::ExplicitRoot,
            mumu_manager_path: PathBuf::from("C:/fixture/nx_main/MuMuManager.exe"),
            version,
            registry_display_version: None,
            instances: vec![
                instance(0, "neutral-a", Some(16384)),
                instance(1, "neutral-b", None),
                instance(2, "neutral-dup", Some(16448)),
                instance(3, "neutral-dup", Some(16480)),
            ],
        }
    }

    /// The one deferred instance of a configuration whose instance declares `binding`.
    fn deferred_instance(binding: Value) -> DeferredInstance {
        let id = IdentifierIssuer::new()
            .expect("issuer")
            .mint_instance_id()
            .expect("instance id");
        let mut instance = json!({
            "alias": "neutral.deferred", "instance_id": id.transport(),
            "application_id": "neutral.application", "connect": false,
            "touch_backend": "adb_shell_input", "capture_backend": "adb"
        });
        for (key, value) in binding.as_object().expect("binding fields") {
            instance[key] = value.clone();
        }
        let config = serde_json::from_value::<ActingdConfigFile>(json!({
            "schema_version": CONFIG_SCHEMA_VERSION,
            "state_root": "state",
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [instance]
        }))
        .expect("typed config");
        let mut deferred = config
            .assemble()
            .expect("runtime assembly")
            .provider
            .deferred;
        assert_eq!(deferred.len(), 1, "one discovery-bound instance");
        deferred.remove(0)
    }

    /// Workflow #378 A3 (acceptance CA-H1, CA-M1): each deferred-instance refusal records its
    /// registered code with the instance's alias and the declared value as values, and what
    /// discovery reported for the instance as `instances` records (the candidates of an
    /// ambiguous name) or, for the adb path, `other_path`. Nothing else is recorded: no serial
    /// and no Runtime text.
    #[test]
    fn a_deferred_instance_refusal_records_the_alias_and_the_declared_and_discovered_values() {
        let report = discovery_report();
        let profile = mumu_capability_profile(&report).expect("admitted capability profile");
        let declared_adb = "C:/fixture/other/adb.exe";
        let cases = [
            (
                json!({"instance_index": 7}),
                ActingdCode::InstanceDiscoveryNoMatch,
                ActingdCode::InstanceDiscoveryNoMatch,
                json!({"alias": "neutral.deferred", "instance_index": 7}),
            ),
            (
                json!({"instance_name": "neutral-dup"}),
                ActingdCode::InstanceDiscoveryAmbiguous,
                ActingdCode::InstanceDiscoveryAmbiguous,
                json!({
                    "alias": "neutral.deferred",
                    "instance_name": "neutral-dup",
                    "instances": [
                        {"instance_index": 2, "instance_name": "neutral-dup"},
                        {"instance_index": 3, "instance_name": "neutral-dup"}
                    ]
                }),
            ),
            (
                json!({"instance_index": 0, "adb_path": declared_adb}),
                ActingdCode::InstanceDiscoveryConflict,
                ActingdCode::AdbPathConflict,
                json!({
                    "alias": "neutral.deferred",
                    "adb_path": declared_adb,
                    "other_path": "C:/fixture/nx_main/adb.exe",
                    "instances": [{"instance_index": 0, "instance_name": "neutral-a"}]
                }),
            ),
            (
                json!({"instance_index": 0, "host": "localhost"}),
                ActingdCode::InstanceDiscoveryConflict,
                ActingdCode::HostConflict,
                json!({
                    "alias": "neutral.deferred",
                    "adb_host": "localhost",
                    "instances": [
                        {"instance_index": 0, "instance_name": "neutral-a", "adb_host": "127.0.0.1"}
                    ]
                }),
            ),
            (
                json!({"instance_index": 0, "port": 16416}),
                ActingdCode::InstanceDiscoveryConflict,
                ActingdCode::PortConflict,
                json!({
                    "alias": "neutral.deferred",
                    "adb_port": 16416,
                    "instances": [
                        {"instance_index": 0, "instance_name": "neutral-a", "adb_port": 16384}
                    ]
                }),
            ),
            (
                json!({"instance_index": 1, "port": 16416}),
                ActingdCode::InstanceDiscoveryConflict,
                ActingdCode::PortUnverifiable,
                json!({
                    "alias": "neutral.deferred",
                    "adb_port": 16416,
                    "instances": [
                        {"instance_index": 1, "instance_name": "neutral-b", "running": false}
                    ]
                }),
            ),
        ];
        for (binding, startup_code, refusal, values) in cases {
            let label = refusal.as_str();
            let Err((code, failure)) =
                resolve_deferred_instance(deferred_instance(binding), &report, &profile)
            else {
                panic!("{label}: the deferred instance must be refused");
            };
            assert_eq!(code, startup_code.code(), "{label}");
            assert!(failure.outcome.is(refusal), "{label}: {}", failure.outcome);
            assert_eq!(
                Value::Object(failure.outcome.values().clone()),
                values,
                "{label}"
            );
            assert!(failure.outcome.causes().is_empty(), "{label}");
        }
    }

    // Workflow #378 A7 (M378 §3.3 and §11; A6a left the MuMu note's details to A7): a MuMu
    // resolution reason, as provider startup records it and check-config notes it, carries the
    // context its resolving step recorded as values from the key table, each only when the step
    // recorded it; count_known is false for a lower bound.
    #[test]
    fn a7_a_mumu_resolution_reason_carries_its_resolution_context_as_values() {
        use actingcommand_contract::outcome::Category;
        use actingcommand_device::{
            MumuInstallSource, NemuConfiguredAdbClass, NemuResolutionCountKind,
            NemuResolutionReason,
        };
        let cases = [
            (
                NemuResolutionReason::TargetProcessAmbiguous,
                NemuResolutionContext::new(NemuResolutionReason::TargetProcessAmbiguous)
                    .with_count(NemuResolutionCountKind::MatchedTargetProcesses, 2, true)
                    .with_source(MumuInstallSource::RunningProcess)
                    .with_provenance(Some(NemuConfiguredAdbClass::SharedMumu), true, false),
                json!({
                    "count_kind": "matched_target_processes",
                    "count": 2,
                    "count_known": false,
                    "install_source": "running_process",
                    "configured_adb_class": "shared_mumu",
                    "explicit_root": true,
                    "explicit_dll": false,
                }),
            ),
            (
                NemuResolutionReason::InstallationAbsent,
                NemuResolutionContext::new(NemuResolutionReason::InstallationAbsent)
                    .with_count(NemuResolutionCountKind::InstallationRoots, 0, false)
                    .with_source(MumuInstallSource::VendorEnumeration),
                json!({
                    "count_kind": "installation_roots",
                    "count": 0,
                    "count_known": true,
                    "install_source": "vendor_enumeration",
                }),
            ),
            (
                NemuResolutionReason::TargetIdentityMismatch,
                NemuResolutionContext::new(NemuResolutionReason::TargetIdentityMismatch),
                json!({}),
            ),
        ];
        for (reason, context, values) in cases {
            let label = reason.as_str();
            let error = DeviceError::fatal("neutral resolution failure")
                .with_nemu_resolution_context_if_absent(context);
            let outcome = device_failure_outcome(&error, None);
            assert!(outcome.is(reason), "{label}: {outcome}");
            assert_eq!(outcome.category(), Category::Error, "{label}");
            assert_eq!(Value::Object(outcome.values().clone()), values, "{label}");
        }
    }
}
