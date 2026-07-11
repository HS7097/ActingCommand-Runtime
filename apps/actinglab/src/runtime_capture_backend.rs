// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{EventActor, EventSource, RuntimeCaptureBackend, RuntimeResult};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendName, DeviceError,
    DeviceResult, Frame,
};
use actingcommand_lab::{
    CaptureBackendObservation, CaptureBackendReport, CaptureBackendRequest, LabError,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct RuntimeCaptureEndpoint {
    instance_alias: String,
    state_root: PathBuf,
}

impl RuntimeCaptureEndpoint {
    pub(super) fn new(instance_alias: String, state_root: PathBuf) -> Self {
        Self {
            instance_alias,
            state_root,
        }
    }
}

pub(super) fn open_runtime_capture(
    endpoint: RuntimeCaptureEndpoint,
    request: CaptureBackendRequest,
) -> Result<Box<dyn CaptureBackend>, LabError> {
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &endpoint.state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(|error| LabError::device(error.to_string()))?;
    Ok(Box::new(RuntimeObservationCaptureBackend {
        client,
        endpoint,
        requested: request.config.requested,
        observation: request.observation,
    }))
}

struct RuntimeObservationCaptureBackend {
    client: RuntimeClient,
    endpoint: RuntimeCaptureEndpoint,
    requested: CaptureBackendChoice,
    observation: Option<CaptureBackendObservation>,
}

impl CaptureBackend for RuntimeObservationCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let started = Instant::now();
        let output = self
            .client
            .observe_readonly(&self.endpoint.instance_alias)
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        let observation = match output.receipt().result() {
            Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => observation,
            _ => {
                return Err(DeviceError::fatal(
                    "Runtime returned an invalid read-only observation receipt",
                ));
            }
        };
        let backend_name = device_backend_name(observation.capture_backend());
        let png = read_projected_verified(&self.endpoint.state_root, observation.artifact())
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        let frame = Frame::from_png(png, backend_name)?;
        if (frame.width, frame.height) != (observation.width(), observation.height()) {
            return Err(DeviceError::fatal(
                "Runtime observation dimensions do not match the verified artifact",
            ));
        }
        self.publish_report(backend_name, started.elapsed().as_millis())?;
        Ok(frame)
    }
}

impl RuntimeObservationCaptureBackend {
    fn publish_report(&self, used: CaptureBackendName, elapsed_ms: u128) -> DeviceResult<()> {
        let Some(observation) = &self.observation else {
            return Ok(());
        };
        observation
            .record(CaptureBackendReport {
                requested: self.requested,
                used,
                attempts: vec![CaptureBackendAttempt {
                    backend: used,
                    ok: true,
                    message: "Runtime observation completed".to_string(),
                    elapsed_ms: Some(elapsed_ms),
                    cached: false,
                    channel_order_contract: channel_order_contract(used),
                    vendor_stdio: Vec::new(),
                }],
            })
            .map_err(|error| DeviceError::fatal(error.to_string()))
    }
}

const fn device_backend_name(backend: RuntimeCaptureBackend) -> CaptureBackendName {
    match backend {
        RuntimeCaptureBackend::AdbScreencap => CaptureBackendName::AdbScreencap,
        RuntimeCaptureBackend::AdbScreencapEncode => CaptureBackendName::AdbScreencapEncode,
        RuntimeCaptureBackend::AdbScreencapRawGzip => CaptureBackendName::AdbScreencapRawGzip,
        RuntimeCaptureBackend::DroidcastRaw => CaptureBackendName::DroidcastRaw,
        RuntimeCaptureBackend::NemuIpc => CaptureBackendName::NemuIpc,
    }
}

const fn channel_order_contract(backend: CaptureBackendName) -> &'static str {
    match backend {
        CaptureBackendName::NemuIpc => "mumu_nemu_verified",
        _ => "verified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{IdentifierIssuer, InstanceId};
    use actingcommand_device::{
        AdbConfig, CaptureBackendConfig, DeviceTarget, InputBackend, PixelFormat,
    };
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    struct SealedProvider {
        instance_id: InstanceId,
    }

    struct SealedCapture;

    impl CaptureBackend for SealedCapture {
        fn capture(&mut self) -> DeviceResult<Frame> {
            Frame::from_pixels(
                2,
                1,
                vec![255, 0, 0, 0, 255, 0],
                PixelFormat::Rgb8,
                CaptureBackendName::NemuIpc,
            )
        }
    }

    impl ExecutionBackendProvider for SealedProvider {
        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == "ak.cn")
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "sealed-device"))
        }

        fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
            Err(DeviceError::fatal(
                "read-only adapter test must not open input",
            ))
        }

        fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
            if instance_alias != "ak.cn" {
                return Err(DeviceError::fatal("unexpected sealed instance"));
            }
            Ok(Box::new(SealedCapture))
        }
    }

    #[test]
    fn runtime_backend_mapping_is_closed_and_lossless() {
        let mappings = [
            (
                RuntimeCaptureBackend::AdbScreencap,
                CaptureBackendName::AdbScreencap,
            ),
            (
                RuntimeCaptureBackend::AdbScreencapEncode,
                CaptureBackendName::AdbScreencapEncode,
            ),
            (
                RuntimeCaptureBackend::AdbScreencapRawGzip,
                CaptureBackendName::AdbScreencapRawGzip,
            ),
            (
                RuntimeCaptureBackend::DroidcastRaw,
                CaptureBackendName::DroidcastRaw,
            ),
            (RuntimeCaptureBackend::NemuIpc, CaptureBackendName::NemuIpc),
        ];
        for (runtime, device) in mappings {
            assert_eq!(device_backend_name(runtime), device);
        }
    }

    #[test]
    fn runtime_adapter_reads_verified_daemon_artifact_over_ipc() {
        let root = TempDir::new().expect("tempdir");
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"actinglab-runtime-capture-test"),
            Arc::new(SealedProvider { instance_id }),
        )
        .expect("Runtime host");
        let endpoint = RuntimeCaptureEndpoint::new("ak.cn".to_string(), root.path().to_path_buf());
        let report = CaptureBackendObservation::default();
        let mut backend = open_runtime_capture(
            endpoint,
            CaptureBackendRequest {
                config: CaptureBackendConfig::new(AdbConfig::default(), DeviceTarget::default()),
                observation: Some(report.clone()),
            },
        )
        .expect("Runtime capture adapter");

        let frame = backend.capture().expect("verified Runtime frame");

        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.backend_name, CaptureBackendName::NemuIpc);
        assert_eq!(
            report.snapshot().expect("capture report").used,
            CaptureBackendName::NemuIpc
        );
        drop(backend);
        host.close().expect("close Runtime host");
    }
}
