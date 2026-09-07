// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ProviderBackend, ProviderNativeFailure, ProviderStartupObservation as Observation,
    ProviderStartupStage as Stage,
};
use actingcommand_runtime_host::{ProviderStartup, RuntimeHostResult};
use actingcommand_vision_ffi::{FastDeployPpocrBackend, OnnxRuntimeBackend, VisionFfiError};
use std::io;

impl ConfiguredExecutionBackendRegistry {
    pub(crate) fn assemble_provider(
        mut self,
        startup: &mut ProviderStartup<'_>,
    ) -> RuntimeHostResult<Arc<dyn ExecutionBackendProvider>> {
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
