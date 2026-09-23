// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    DiscoveredInstanceObservation, ProviderBackend, ProviderNativeFailure,
    ProviderStartupObservation as Observation, ProviderStartupStage as Stage,
};
use actingcommand_device::{
    DiscoveredMumuInstance, EmulatorCapability, EmulatorCapabilityAvailability,
    EmulatorCapabilityProfile, MUMU_CAPABILITY_PROVIDER_ID, MumuDiscoveryReport,
    MumuEmulatorCapabilityBackend, NemuResolutionReason, discover_mumu_instances,
};
use actingcommand_execution_kernel::{
    InstanceDiscoveryFailure, ProviderDiscoveredInstance, ProviderInstanceDiscovery,
};
use actingcommand_runtime_host::{
    DiscoveredInstanceBinding, ProviderStartup, RuntimeHostResult, admit_emulator_capabilities,
};
use actingcommand_vision_ffi::{FastDeployPpocrBackend, OnnxRuntimeBackend, VisionFfiError};
use std::io;

impl ConfiguredExecutionBackendRegistry {
    pub(crate) fn assemble_provider(
        mut self,
        startup: &mut ProviderStartup<'_>,
    ) -> RuntimeHostResult<Arc<dyn ExecutionBackendProvider>> {
        if !self.deferred.is_empty() {
            self.bind_discovered_instances(startup)?;
        }
        let Some((source_root, configured_path)) = self.pending_vision.take() else {
            startup.record(ProviderBackend::Configured, Observation::NotConfigured)?;
            return Ok(Arc::new(self));
        };
        let provider = assemble_vision_provider(startup, &source_root, &configured_path)?;
        if let Some(registry) = self.devices.take() {
            self.devices = Some(registry.with_vision_provider(Arc::clone(&provider)));
        }
        if let Some(registry) = &mut self.fixtures {
            registry.vision_provider = Some(provider);
        }
        startup.record(ProviderBackend::Configured, Observation::Ready)?;
        Ok(Arc::new(self))
    }

    /// Runs `MuMuManager` discovery once per startup and registers every deferred instance.
    /// Every refusal is recorded as a Provider startup failure before startup fails.
    fn bind_discovered_instances(
        &mut self,
        startup: &mut ProviderStartup<'_>,
    ) -> RuntimeHostResult<()> {
        let backend = ProviderBackend::MumuManager;
        let stage = Stage::InstanceDiscovery;
        startup.record(backend, Observation::Started { stage })?;
        let mumu_root = self
            .mumu_root
            .as_deref()
            .map_or_else(|| "<unset>".to_owned(), |root| root.display().to_string());
        let report = discover_mumu_instances(self.mumu_root.as_deref()).map_err(|error| {
            let classification = discovery_refusal_code(&error);
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
        for entry in std::mem::take(&mut self.deferred) {
            let alias = entry.alias.clone();
            match resolve_deferred_instance(entry, &report, &profile) {
                Ok((instance_index, device)) => {
                    bound.insert(instance_index, alias);
                    resolved.push(device);
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
        for device in resolved {
            self.register_device(device).map_err(|code| {
                startup.failed(
                    backend,
                    Stage::RegistryBinding,
                    code,
                    ProviderNativeFailure {
                        module: "actingd.config".into(),
                        code: code.into(),
                        severity: "fatal".into(),
                        message: "discovered instance registration was refused".into(),
                    },
                )
            })?;
        }
        startup.record(backend, Observation::Completed { stage })
    }

    /// On-demand discovery: the same `MuMuManager` resolution as startup, mapped to the
    /// provider view. Binds, rebinds, registers and records nothing.
    pub(super) fn discover_instances_on_demand(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        let report = discover_mumu_instances(self.mumu_root.as_deref()).map_err(|error| {
            Box::new(InstanceDiscoveryFailure {
                code: discovery_refusal_code(&error),
                error,
            })
        })?;
        Ok(ProviderInstanceDiscovery {
            provider_version: report.version.to_string(),
            instances: report
                .instances
                .into_iter()
                .map(|instance| ProviderDiscoveredInstance {
                    instance_index: instance.instance_index,
                    instance_name: instance.instance_name,
                    adb_host: instance.adb_host,
                    adb_port: instance.adb_port,
                    running: instance.running,
                    android_version: instance.android_version,
                })
                .collect(),
        })
    }
}

/// Classifies one discovery refusal, at startup and on demand: a version below the policy
/// floor or unparseable is `mumu_manager_version_unsupported`, anything else (no install,
/// tool spawn, exit, timeout, decode or JSON failure) `instance_discovery_unavailable`.
fn discovery_refusal_code(error: &DeviceError) -> &'static str {
    if matches!(
        error
            .nemu_resolution_context()
            .map(|context| context.reason()),
        Some(
            NemuResolutionReason::ProviderVersionBelowMinimum
                | NemuResolutionReason::ProviderVersionUnparseable
        )
    ) {
        "mumu_manager_version_unsupported"
    } else {
        "instance_discovery_unavailable"
    }
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
) -> Result<(u16, ConfiguredInstanceBackend), DeferredRefusal> {
    let DeferredInstance { alias, key, config } = entry;
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
    if let Some(declared) = config.adb_path.as_deref()
        && !fs::canonicalize(declared).is_ok_and(|path| path == instance.adb_path)
    {
        return Err((
            "instance_discovery_conflict",
            failure(
                "adb_path_conflict",
                format!("{facts}; declared adb_path={declared:?}; {discovered}"),
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
    let adb_path = instance.adb_path.to_str().ok_or_else(|| {
        (
            "instance_discovery_unavailable",
            failure(
                "adb_path_encoding_invalid",
                format!("{facts}; {discovered}"),
            ),
        )
    })?;
    let binding = DiscoveredInstanceBinding::new(
        instance.instance_index,
        instance.instance_name.clone(),
        report.version.to_string(),
        instance.mumu_manager_path.clone(),
    );
    let adb_port = instance.adb_port;
    let device = config
        .device_registration(adb_path.to_owned(), adb_host, adb_port)
        .map_err(|code| (code, failure(code, format!("{facts}; {discovered}"))))?;
    let ConfiguredInstanceBackend::Device {
        alias,
        instance_id,
        input_backend,
        capture_backend,
        registration,
    } = device
    else {
        return Err((
            "instance_registration_invalid",
            failure("instance_registration_invalid", facts),
        ));
    };
    Ok((
        instance.instance_index,
        ConfiguredInstanceBackend::Device {
            alias,
            instance_id,
            input_backend,
            capture_backend,
            registration: Box::new(
                match adb_port {
                    Some(_) => registration.with_discovered_binding(binding),
                    None => registration.with_pending_discovered_binding(binding),
                }
                .with_capability_profile(profile.clone()),
            ),
        },
    ))
}

fn assemble_vision_provider(
    startup: &mut ProviderStartup<'_>,
    source_root: &Path,
    configured_path: &Path,
) -> RuntimeHostResult<Arc<dyn RecognitionVisionProvider>> {
    let backend = ProviderBackend::Configured;
    startup.record(
        backend,
        Observation::Started {
            stage: Stage::ManifestRead,
        },
    )?;
    let read = || -> Result<_, (String, String)> {
        if configured_path.as_os_str().is_empty() {
            return Err(("invalid_path".into(), "empty provider manifest path".into()));
        }
        let path = if configured_path.is_absolute() {
            configured_path.to_path_buf()
        } else {
            source_root.join(configured_path)
        };
        let io_error = |error: io::Error| (format!("{:?}", error.kind()), error.to_string());
        let path = fs::canonicalize(path).map_err(io_error)?;
        let metadata = fs::metadata(&path).map_err(io_error)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_VISION_MANIFEST_BYTES
        {
            return Err((
                "invalid_size".into(),
                format!("manifest bytes: {}", metadata.len()),
            ));
        }
        let bytes = fs::read(&path).map_err(io_error)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_VISION_MANIFEST_BYTES {
            return Err((
                "invalid_size".into(),
                format!("manifest bytes: {}", bytes.len()),
            ));
        }
        Ok((path, bytes))
    };
    let (manifest_path, bytes) = read().map_err(|(code, message)| {
        let classification = match code.as_str() {
            "invalid_path" => "vision_provider_manifest_invalid",
            "invalid_size" => "vision_provider_manifest_size_invalid",
            _ => "vision_provider_manifest_unavailable",
        };
        startup.failed(
            backend,
            Stage::ManifestRead,
            classification,
            ProviderNativeFailure {
                module: "actingd.config".into(),
                code,
                severity: "fatal".into(),
                message: format!(
                    "manifest={configured_path:?}; source_root={source_root:?}; {message}"
                ),
            },
        )
    })?;
    startup.record(
        backend,
        Observation::Completed {
            stage: Stage::ManifestRead,
        },
    )?;
    binding(
        startup,
        backend,
        "manifest",
        configured_path,
        source_root,
        &manifest_path,
    )?;
    startup.record(
        backend,
        Observation::Started {
            stage: Stage::ManifestParse,
        },
    )?;
    let mut manifest =
        VisionProviderArtifactManifest::from_json_slice(&bytes).map_err(|error| {
            ffi_failure(
                startup,
                backend,
                Stage::ManifestParse,
                "vision_provider_manifest_invalid",
                error,
            )
        })?;
    if manifest.schema_version != VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION {
        return Err(startup.failed(
            backend,
            Stage::ManifestParse,
            "vision_provider_manifest_invalid",
            ProviderNativeFailure {
                module: "actingd.config".into(),
                code: "schema_mismatch".into(),
                severity: "fatal".into(),
                message: manifest.schema_version,
            },
        ));
    }
    startup.record(
        backend,
        Observation::Completed {
            stage: Stage::ManifestParse,
        },
    )?;
    // Canonical file paths always have a parent. Preserve explicit failure if this invariant fails.
    let artifact_root = manifest_path.parent().ok_or_else(|| {
        startup.failed(
            backend,
            Stage::PathBinding,
            "vision_provider_manifest_invalid",
            ProviderNativeFailure {
                module: "actingd.config".into(),
                code: "manifest_parent_missing".into(),
                severity: "fatal".into(),
                message: format!("{manifest_path:?}"),
            },
        )
    })?;
    let configured = manifest.clone();
    resolve_vision_artifact_paths(&mut manifest, artifact_root);
    record_bindings(startup, &configured, &manifest, artifact_root)?;

    if manifest.fastdeploy_ppocr.is_none() {
        startup.record(ProviderBackend::FastdeployPpocr, Observation::NotConfigured)?;
    }
    if manifest.onnxruntime.is_none() {
        startup.record(ProviderBackend::Onnxruntime, Observation::NotConfigured)?;
    }

    let ocr = manifest
        .fastdeploy_ppocr
        .take()
        .map(|artifacts| {
            let backend = ProviderBackend::FastdeployPpocr;
            startup.record(
                backend,
                Observation::Started {
                    stage: Stage::ModelIdentity,
                },
            )?;
            let (model_ref, model_sha256) =
                artifacts.production_model_identity().map_err(|error| {
                    ffi_failure(
                        startup,
                        backend,
                        Stage::ModelIdentity,
                        "vision_provider_manifest_invalid",
                        error,
                    )
                })?;
            startup.record(
                backend,
                Observation::ModelBinding {
                    model_ref: model_ref.into(),
                    model_sha256: model_sha256.into(),
                },
            )?;
            let identity = VisionModelIdentity::new(model_ref, model_sha256).map_err(|error| {
                kernel_failure(
                    startup,
                    backend,
                    Stage::ModelIdentity,
                    (format!("{:?}", error.code()), error.message().into()),
                )
            })?;
            startup.record(
                backend,
                Observation::Completed {
                    stage: Stage::ModelIdentity,
                },
            )?;
            startup.record(
                backend,
                Observation::Started {
                    stage: Stage::BackendConstruction,
                },
            )?;
            let engine = FastDeployPpocrBackend::from_artifacts(artifacts).map_err(|error| {
                ffi_failure(
                    startup,
                    backend,
                    Stage::BackendConstruction,
                    "vision_provider_unavailable",
                    error,
                )
            })?;
            startup.record(
                backend,
                Observation::Completed {
                    stage: Stage::BackendConstruction,
                },
            )?;
            Ok::<_, actingcommand_runtime_host::RuntimeHostError>((
                Box::new(engine) as Box<dyn OcrEngine + Send>,
                identity,
            ))
        })
        .transpose()?;
    let nn = manifest
        .onnxruntime
        .take()
        .map(|artifacts| {
            let backend = ProviderBackend::Onnxruntime;
            startup.record(
                backend,
                Observation::Started {
                    stage: Stage::ModelIdentity,
                },
            )?;
            let (model_ref, model_sha256) =
                artifacts.production_model_identity().map_err(|error| {
                    ffi_failure(
                        startup,
                        backend,
                        Stage::ModelIdentity,
                        "vision_provider_manifest_invalid",
                        error,
                    )
                })?;
            startup.record(
                backend,
                Observation::ModelBinding {
                    model_ref: model_ref.into(),
                    model_sha256: model_sha256.into(),
                },
            )?;
            let identity = VisionModelIdentity::new(model_ref, model_sha256).map_err(|error| {
                kernel_failure(
                    startup,
                    backend,
                    Stage::ModelIdentity,
                    (format!("{:?}", error.code()), error.message().into()),
                )
            })?;
            startup.record(
                backend,
                Observation::Completed {
                    stage: Stage::ModelIdentity,
                },
            )?;
            startup.record(
                backend,
                Observation::Started {
                    stage: Stage::BackendConstruction,
                },
            )?;
            let engine = OnnxRuntimeBackend::from_artifacts(artifacts).map_err(|error| {
                ffi_failure(
                    startup,
                    backend,
                    Stage::BackendConstruction,
                    "vision_provider_unavailable",
                    error,
                )
            })?;
            startup.record(
                backend,
                Observation::Completed {
                    stage: Stage::BackendConstruction,
                },
            )?;
            Ok::<_, actingcommand_runtime_host::RuntimeHostError>((
                Box::new(engine) as Box<dyn NnEngine + Send>,
                identity,
            ))
        })
        .transpose()?;
    let provider = VisionFfiProvider::new(ocr, nn).map_err(|error| {
        kernel_failure(
            startup,
            backend,
            Stage::RegistryBinding,
            (format!("{:?}", error.code()), error.message().into()),
        )
    })?;
    Ok(Arc::new(provider))
}

fn ffi_failure(
    startup: &mut ProviderStartup<'_>,
    backend: ProviderBackend,
    stage: Stage,
    code: &'static str,
    error: VisionFfiError,
) -> actingcommand_runtime_host::RuntimeHostError {
    startup.failed(
        backend,
        stage,
        code,
        ProviderNativeFailure {
            module: error.module().into(),
            code: format!("{:?}", error.code()),
            severity: format!("{:?}", error.severity()),
            message: error.message().into(),
        },
    )
}

fn kernel_failure(
    startup: &mut ProviderStartup<'_>,
    backend: ProviderBackend,
    stage: Stage,
    error: (String, String),
) -> actingcommand_runtime_host::RuntimeHostError {
    startup.failed(
        backend,
        stage,
        "vision_provider_manifest_invalid",
        ProviderNativeFailure {
            module: "execution-kernel".into(),
            code: error.0,
            severity: "fatal".into(),
            message: error.1,
        },
    )
}

fn binding(
    startup: &mut ProviderStartup<'_>,
    backend: ProviderBackend,
    field: &str,
    configured: &Path,
    base: &Path,
    resolved: &Path,
) -> RuntimeHostResult<()> {
    let path = |value: &Path| {
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
    };
    let mut path = path;
    let configured = path(configured)?;
    let base = path(base)?;
    let resolved = path(resolved)?;
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

fn record_bindings(
    startup: &mut ProviderStartup<'_>,
    configured: &VisionProviderArtifactManifest,
    resolved: &VisionProviderArtifactManifest,
    base: &Path,
) -> RuntimeHostResult<()> {
    if let (Some(a), Some(b)) = (&configured.fastdeploy_ppocr, &resolved.fastdeploy_ppocr) {
        let backend = ProviderBackend::FastdeployPpocr;
        for (field, a, b) in [
            (
                "provider_library_path",
                &a.provider_library_path,
                &b.provider_library_path,
            ),
            (
                "detector_model_path",
                &a.detector_model_path,
                &b.detector_model_path,
            ),
            (
                "recognizer_model_path",
                &a.recognizer_model_path,
                &b.recognizer_model_path,
            ),
            ("dictionary_path", &a.dictionary_path, &b.dictionary_path),
        ] {
            binding(startup, backend, field, a, base, b)?;
        }
        for (index, (a, b)) in a
            .runtime_library_paths
            .iter()
            .zip(&b.runtime_library_paths)
            .enumerate()
        {
            binding(
                startup,
                backend,
                &format!("runtime_library_paths[{index}]"),
                a,
                base,
                b,
            )?;
        }
        for (field, a, b) in [
            (
                "runtime_library_path",
                &a.runtime_library_path,
                &b.runtime_library_path,
            ),
            (
                "classifier_model_path",
                &a.classifier_model_path,
                &b.classifier_model_path,
            ),
        ] {
            if let (Some(a), Some(b)) = (a, b) {
                binding(startup, backend, field, a, base, b)?;
            }
        }
    }
    if let (Some(a), Some(b)) = (&configured.onnxruntime, &resolved.onnxruntime) {
        let backend = ProviderBackend::Onnxruntime;
        for (field, a, b) in [
            (
                "provider_library_path",
                &a.provider_library_path,
                &b.provider_library_path,
            ),
            ("model_path", &a.model_path, &b.model_path),
        ] {
            binding(startup, backend, field, a, base, b)?;
        }
        for (field, a, b) in [
            (
                "runtime_library_path",
                &a.runtime_library_path,
                &b.runtime_library_path,
            ),
            ("labels_path", &a.labels_path, &b.labels_path),
        ] {
            if let (Some(a), Some(b)) = (a, b) {
                binding(startup, backend, field, a, base, b)?;
            }
        }
    }
    Ok(())
}
