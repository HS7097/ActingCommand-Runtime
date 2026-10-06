// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    DiscoveredInstanceObservation, ProviderBackend, ProviderNativeFailure,
    ProviderStartupObservation as Observation, ProviderStartupStage as Stage,
};
use actingcommand_device::{
    DiscoveredMumuInstance, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityProfile, MUMU_CAPABILITY_PROVIDER_ID, MumuDiscoveryReport,
    MumuEmulatorCapabilityBackend,
};
use actingcommand_execution_kernel::InstanceDiscoveryFailure;
use actingcommand_ppocr_onnx_json_provider::PpocrCtcLoader;
use actingcommand_runtime_host::{
    DiscoveredInstanceBinding, ProviderStartup, RuntimeHostResult, admit_emulator_capabilities,
};
use actingcommand_vision_ffi::{ONNXRUNTIME_LIBRARY, VISION_RUNTIME_DIRECTORY};

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
            instances.extend(bind_discovered_instances(startup, &discovery, deferred)?);
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
                    code: error.code().into(),
                    severity: "fatal".into(),
                    message: "execution backend registration was refused".into(),
                },
            )
        })?;
        if vision_configured {
            startup.record(ProviderBackend::Configured, Observation::Ready)?;
        }
        Ok(Arc::new(registry))
    }
}

/// Runs `MuMuManager` discovery once per startup and completes every deferred instance into
/// its registration; the registry registers them together with the explicit ones. Every
/// refusal is recorded as a Provider startup failure before startup fails.
fn bind_discovered_instances(
    startup: &mut ProviderStartup<'_>,
    discovery: &DiscoverySpec,
    deferred: Vec<DeferredInstance>,
) -> RuntimeHostResult<Vec<InstanceSpec>> {
    let backend = ProviderBackend::MumuManager;
    let stage = Stage::InstanceDiscovery;
    startup.record(backend, Observation::Started { stage })?;
    let mumu_root = discovery
        .mumu_root()
        .map_or_else(|| "<unset>".to_owned(), |root| root.display().to_string());
    let report = discovery.discover().map_err(|failure| {
        let InstanceDiscoveryFailure {
            code: classification,
            error,
        } = *failure;
        let code = error
            .nemu_resolution_context()
            .map(|context| context.reason().as_str().to_owned())
            .or_else(|| {
                error.diagnostic().map(|diagnostic| {
                    format!("{}.{}", diagnostic.category().as_str(), diagnostic.stage())
                })
            })
            .unwrap_or_else(|| "device_error".to_owned());
        startup.failed(
            backend,
            stage,
            classification,
            ProviderNativeFailure {
                module: "actingcommand_device::mumu_manager".into(),
                code,
                severity: format!("{:?}", error.severity()).to_ascii_lowercase(),
                message: format!(
                    "mumu_root={mumu_root}; {}; diagnostic={}",
                    error.message(),
                    error.diagnostic_message().unwrap_or("unavailable")
                ),
            },
        )
    })?;
    let profile = admit_capability_profile(startup, &report, &mumu_root)?;
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
        return Err(startup.failed(backend, stage, classification, failure));
    }
    startup.record(backend, Observation::Completed { stage })?;
    Ok(resolved)
}

/// Admits the capability profile derived from the discovery report (pure, nothing is
/// dispatched) and records it as one `capability_profile` observation inside a
/// `capability_admission` bracket. A refusal is recorded before startup fails.
fn admit_capability_profile(
    startup: &mut ProviderStartup<'_>,
    report: &MumuDiscoveryReport,
    mumu_root: &str,
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
        startup.failed(
            backend,
            stage,
            "emulator_capability_admission_refused",
            ProviderNativeFailure {
                module: "actingcommand_runtime_host::emulator_control".into(),
                code: error.code().into(),
                severity: "fatal".into(),
                message: format!(
                    "mumu_root={mumu_root}; provider_id={MUMU_CAPABILITY_PROVIDER_ID}; version={}; {error}",
                    report.version
                ),
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

fn render_discovered_instances(instances: &[&DiscoveredMumuInstance]) -> String {
    let rendered = instances
        .iter()
        .map(|instance| {
            format!(
                "{{index={} name={:?} adb_host={:?} adb_port={:?} running={} player_state={:?}}}",
                instance.instance_index,
                instance.instance_name,
                instance.adb_host,
                instance.adb_port,
                instance.running,
                instance.player_state
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", rendered.join(", "))
}

type DeferredRefusal = (&'static str, ProviderNativeFailure);

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
    let facts = format!(
        "alias={alias} {key} source={} mumu_manager_path={} version={}",
        report.source.as_str(),
        report.mumu_manager_path.display(),
        report.version
    );
    let failure = |code: &str, message: String| ProviderNativeFailure {
        module: "actingd.config".into(),
        code: code.into(),
        severity: "fatal".into(),
        message,
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
            let discovered = report.instances.iter().collect::<Vec<_>>();
            return Err((
                "instance_discovery_no_match",
                failure(
                    "no_match",
                    format!(
                        "{facts}; discovered={}",
                        render_discovered_instances(&discovered)
                    ),
                ),
            ));
        }
        [instance] => *instance,
        _ => {
            return Err((
                "instance_discovery_ambiguous",
                failure(
                    "ambiguous",
                    format!("{facts}; matches={}", render_discovered_instances(&matches)),
                ),
            ));
        }
    };
    let discovered = format!(
        "discovered index={} name={:?} running={} adb_host={:?} adb_port={:?} adb_path={}",
        instance.instance_index,
        instance.instance_name,
        instance.running,
        instance.adb_host,
        instance.adb_port,
        instance.adb_path.display()
    );
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
        let accepted = match config.adb.installed() {
            Some(installed) => format!(
                "the discovered MuMu adb {} or this install root's adb {} (the default when adb_path is omitted)",
                instance.adb_path.display(),
                installed.path_string()
            ),
            None => format!(
                "the discovered MuMu adb {} (the default when adb_path is omitted; the daemon does not run from an AC install root, so there is no AC adb)",
                instance.adb_path.display()
            ),
        };
        return Err((
            "instance_discovery_conflict",
            failure(
                "adb_path_conflict",
                format!(
                    "{facts}; declared adb_path={declared:?}; {discovered}; accepted adb_path: {accepted}"
                ),
            ),
        ));
    }
    if let (Some(declared), Some(adb_host)) = (config.host.as_deref(), instance.adb_host.as_deref())
        && declared.trim() != adb_host
    {
        return Err((
            "instance_discovery_conflict",
            failure(
                "host_conflict",
                format!("{facts}; declared host={declared:?}; {discovered}"),
            ),
        ));
    }
    // A stopped instance reports no ADB endpoint (observed on MuMuManager 6.5.7.0): it is
    // bound PENDING, never with a guessed port, and `emulator start` resolves the port. A
    // declared port cannot be cross-checked then and is refused rather than trusted.
    match (config.port, instance.adb_port) {
        (Some(declared), Some(adb_port)) if declared != adb_port => {
            return Err((
                "instance_discovery_conflict",
                failure(
                    "port_conflict",
                    format!("{facts}; declared port={declared}; {discovered}"),
                ),
            ));
        }
        (Some(declared), None) => {
            return Err((
                "instance_discovery_conflict",
                failure(
                    "port_unverifiable",
                    format!(
                        "{facts}; declared port={declared}; {discovered}; the instance is stopped, so the declared port cannot be cross-checked: omit `port` to bind it pending"
                    ),
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
                (
                    "instance_discovery_unavailable",
                    failure(
                        "adb_path_encoding_invalid",
                        format!("{facts}; {discovered}"),
                    ),
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
        .map_err(|code| (code, failure(code, format!("{facts}; {discovered}"))))?;
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
            refusal.code,
            ProviderNativeFailure {
                module: "actingd.config".into(),
                code: refusal.code.into(),
                severity: "fatal".into(),
                message: refusal.message,
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
    // An invalid folder's name or path may not be UTF-8; its fact carries the lossy form.
    for folder in &inventory.listing.invalid {
        startup.record(
            backend,
            Observation::Binding {
                field: format!("invalid_model:{}", folder.name),
                configured: format!("invalid: {}", folder.reason),
                base: models_dir.clone(),
                resolved: folder.path.to_string_lossy().into_owned(),
            },
        )?;
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
            "vision_provider_unavailable",
            ProviderNativeFailure {
                module: "execution-kernel".into(),
                code: format!("{:?}", error.code()),
                severity: "fatal".into(),
                message: error.message().into(),
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
            "vision_provider_path_encoding_invalid",
            ProviderNativeFailure {
                module: "actingd.config".into(),
                code: "path_encoding_invalid".into(),
                severity: "fatal".into(),
                message: format!("{value:?}"),
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
