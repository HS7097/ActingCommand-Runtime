// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    CaptureSequenceSpec, EventActor, EventSource, ReadonlyObservation, RuntimeCaptureBackend,
    RuntimeResult,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendName, DeviceError,
    DeviceResult, Frame,
};
use actingcommand_lab::{
    CaptureBackendObservation, CaptureBackendReport, CaptureBackendRequest, LabError,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

/// Captures a bounded frame sequence through the resident Runtime without opening a client-side
/// device backend.
pub(super) fn capture_runtime_sequence(
    endpoint: &RuntimeCaptureEndpoint,
    frame_count: u16,
    interval: Duration,
) -> DeviceResult<Vec<Frame>> {
    let interval_ms = u64::try_from(interval.as_millis())
        .map_err(|_| DeviceError::fatal("Runtime capture interval exceeds u64 milliseconds"))?;
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &endpoint.state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(|error| DeviceError::fatal(error.to_string()))?;
    let observations = if frame_count == 1 {
        let output = client
            .observe_readonly(&endpoint.instance_alias)
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        match output.receipt().result() {
            Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => {
                vec![observation.clone()]
            }
            _ => {
                return Err(DeviceError::fatal(
                    "Runtime returned an invalid read-only observation receipt",
                ));
            }
        }
    } else {
        let spec = CaptureSequenceSpec::new(frame_count, interval_ms)
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        let output = client
            .capture_sequence(&endpoint.instance_alias, spec)
            .map_err(|error| DeviceError::fatal(error.to_string()))?;
        match output.receipt().result() {
            Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => {
                sequence.observations().to_vec()
            }
            _ => {
                return Err(DeviceError::fatal(
                    "Runtime returned an invalid capture sequence receipt",
                ));
            }
        }
    };
    observations
        .iter()
        .map(|observation| frame_from_observation(&endpoint.state_root, observation))
        .collect()
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
    let mut backend = RuntimeObservationCaptureBackend {
        client,
        endpoint,
        requested: request.config.requested,
        observation: request.observation,
        pending_frame: None,
    };
    // The Lab port requires truthful backend diagnostics at open time, so acquire the first
    // Runtime-owned observation once and return that same frame on the first capture call.
    let started = Instant::now();
    let frame = backend
        .capture_runtime_frame()
        .map_err(|error| LabError::device(error.to_string()))?;
    backend
        .publish_report(frame.backend_name, started.elapsed().as_millis())
        .map_err(|error| LabError::device(error.to_string()))?;
    backend.pending_frame = Some(frame);
    Ok(Box::new(backend))
}

struct RuntimeObservationCaptureBackend {
    client: RuntimeClient,
    endpoint: RuntimeCaptureEndpoint,
    requested: CaptureBackendChoice,
    observation: Option<CaptureBackendObservation>,
    pending_frame: Option<Frame>,
}

impl CaptureBackend for RuntimeObservationCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        if let Some(frame) = self.pending_frame.take() {
            return Ok(frame);
        }
        let started = Instant::now();
        let frame = self.capture_runtime_frame()?;
        self.publish_report(frame.backend_name, started.elapsed().as_millis())?;
        Ok(frame)
    }
}

impl RuntimeObservationCaptureBackend {
    fn capture_runtime_frame(&self) -> DeviceResult<Frame> {
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
        frame_from_observation(&self.endpoint.state_root, observation)
    }

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

pub(super) fn frame_from_observation(
    state_root: &std::path::Path,
    observation: &ReadonlyObservation,
) -> DeviceResult<Frame> {
    let backend_name = device_backend_name(observation.capture_backend());
    let png = read_projected_verified(state_root, observation.artifact())
        .map_err(|error| DeviceError::fatal(error.to_string()))?;
    let frame = Frame::from_png(png, backend_name)?;
    if (frame.width, frame.height) != (observation.width(), observation.height()) {
        return Err(DeviceError::fatal(
            "Runtime observation dimensions do not match the verified artifact",
        ));
    }
    Ok(frame)
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
        fn instance_aliases(&self) -> Vec<String> {
            vec!["ak.cn".to_string()]
        }

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
                instance_alias: None,
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

    #[test]
    fn runtime_sequence_returns_verified_frames_without_client_backend_authority() {
        let root = TempDir::new().expect("tempdir");
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"actinglab-runtime-sequence-test"),
            Arc::new(SealedProvider { instance_id }),
        )
        .expect("Runtime host");
        let endpoint = RuntimeCaptureEndpoint::new("ak.cn".to_string(), root.path().to_path_buf());

        let frames = capture_runtime_sequence(&endpoint, 2, Duration::from_millis(1))
            .expect("Runtime-owned sequence");

        assert_eq!(frames.len(), 2);
        assert!(
            frames
                .iter()
                .all(|frame| frame.backend_name == CaptureBackendName::NemuIpc)
        );
        host.close().expect("close Runtime host");
    }
}
