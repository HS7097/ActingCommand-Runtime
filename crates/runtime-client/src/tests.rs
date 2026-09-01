// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, ReceiptReadDeadline, exchange};
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, AuditInput, CaptureSequenceSpec,
    ClientActionKind, ClientActionRecord, ContainedTaskRequest, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    IdentifierIssuer, InputAction, InstanceId, LeasePriority, LeaseQueuePolicy,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, OwnerEpoch, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RUNTIME_INFO_FILE, ResourceAuthoringEvent, ResourceAuthoringPhase,
    RuntimeCaptureBackend, RuntimeDebugEvent, RuntimeDebugOperation, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeEventQueryCursor, RuntimeEventQueryPage, RuntimeInfo,
    RuntimeMonitorPolicy, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, RuntimeSubscriptionRequest, SanitizationError, SecretField, SecretFingerprinter,
    Sha256Fingerprint, SubscriptionCursor, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
    TerminalEvent,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_scheduler::SchedulerConfig;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[cfg(feature = "test-observation")]
use crate::test_observation::{
    ObservationIdentities, ObservationOperation, ObservationOutcome, ObservationRecord,
    ObservationStage, ObservationThreadRole, TestObservationCapture, TestObservationOwner,
    TestObservationRecorder, current_observation_owner, enter_observation_owner, record_active,
};
#[cfg(feature = "test-observation")]
use actingcommand_runtime_host::test_observation::{
    HostTestObservation, HostTestObservationGuard, HostTestObservationOperation,
    HostTestObservationOutcome, HostTestObservationPoint,
};
#[cfg(feature = "test-observation")]
use std::collections::BTreeSet;

const TEST_GOVERNANCE_CAPABILITY: &str = "runtime-client-governance-test-capability";

struct RejectProjectionSecrets;

impl SecretFingerprinter for RejectProjectionSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("projection fixture does not contain secrets")
    }
}

fn projected_task_event(issuer: &IdentifierIssuer, sequence: u64) -> ProjectedEvent {
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        sequence,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        TaskPayloadDraft::semantic(TaskSemanticFact::RunStarted, AuditInput::new()).into(),
    )
    .sanitize(&RejectProjectionSecrets)
    .expect("sanitize projection fixture");
    ProjectedEvent {
        schema_version: sanitized.schema_version().to_owned(),
        sequence,
        event_id: *sanitized.event_id(),
        timestamp_unix_ms: sanitized.timestamp_unix_ms(),
        event_type: sanitized.event_type(),
        severity: sanitized.severity(),
        sensitivity: sanitized.sensitivity(),
        origin: sanitized.origin().clone(),
        links: sanitized.links().clone(),
        payload_schema: sanitized.payload_schema().to_owned(),
        payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
        artifacts: Vec::new(),
    }
}

struct CaptureGate {
    entered: Barrier,
    release: Barrier,
}

impl CaptureGate {
    fn new() -> Self {
        Self {
            entered: Barrier::new(2),
            release: Barrier::new(2),
        }
    }
}

#[derive(Default)]
struct FakeState {
    opens: AtomicUsize,
    inputs: AtomicUsize,
    closes: AtomicUsize,
    fail_input: AtomicBool,
    capture_opens: AtomicUsize,
    captures: AtomicUsize,
    capture_closes: AtomicUsize,
    fail_capture: AtomicBool,
    invalid_capture: AtomicBool,
    capture_gate: Option<Arc<CaptureGate>>,
    #[cfg(feature = "test-observation")]
    observation_owner: Option<TestObservationOwner>,
}

impl FakeState {
    #[cfg(feature = "test-observation")]
    fn for_test_observation() -> Self {
        Self {
            observation_owner: current_observation_owner(),
            ..Self::default()
        }
    }

    #[cfg(not(feature = "test-observation"))]
    fn for_test_observation() -> Self {
        Self::default()
    }
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl FakeBackend {
    fn input(&self) -> DeviceResult<()> {
        #[cfg(feature = "test-observation")]
        let _observation_owner = enter_observation_owner(self.state.observation_owner);
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendInputStart,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Started,
            None,
            None,
            None,
        );
        if self.state.fail_input.load(Ordering::Acquire) {
            #[cfg(feature = "test-observation")]
            record_active(
                ObservationStage::BackendInputResult,
                ObservationOperation::Backend,
                ObservationThreadRole::Backend,
                ObservationOutcome::Failure,
                None,
                None,
                None,
            );
            return Err(DeviceError::fatal("injected input failure"));
        }
        self.state.inputs.fetch_add(1, Ordering::AcqRel);
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendInputResult,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Success,
            None,
            None,
            None,
        );
        Ok(())
    }
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.input()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, duration_ms: u64) -> DeviceResult<()> {
        #[cfg(feature = "test-observation")]
        let _observation_owner = enter_observation_owner(self.state.observation_owner);
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendLongInputStart,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Started,
            None,
            None,
            None,
        );
        thread::sleep(Duration::from_millis(duration_ms));
        let result = self.input();
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendLongInputResult,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            None,
        );
        result
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.input()
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.input()
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.input()
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.input()
    }

    fn close(&mut self) -> DeviceResult<()> {
        #[cfg(feature = "test-observation")]
        let _observation_owner = enter_observation_owner(self.state.observation_owner);
        if !self.closed {
            self.closed = true;
            self.state.closes.fetch_add(1, Ordering::AcqRel);
        }
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendClose,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Closed,
            None,
            None,
            None,
        );
        Ok(())
    }
}

struct FakeProvider {
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

struct NeutralProjectProvider {
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

struct FakeCapture {
    state: Arc<FakeState>,
    closed: bool,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.state.captures.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = &self.state.capture_gate {
            gate.entered.wait();
            gate.release.wait();
        }
        if self.state.fail_capture.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected capture failure"));
        }
        if self.state.invalid_capture.load(Ordering::Acquire) {
            return Ok(Frame {
                width: 2,
                height: 1,
                pixels: Vec::new(),
                pixel_format: PixelFormat::Rgb8,
                original_png: None,
                captured_at: std::time::SystemTime::now(),
                backend_name: CaptureBackendName::AdbScreencap,
            });
        }
        Frame::from_pixels(
            2,
            1,
            vec![255, 0, 0, 0, 255, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            self.state.capture_closes.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["node.a".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "node.a")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        #[cfg(feature = "test-observation")]
        let _observation_owner = enter_observation_owner(self.state.observation_owner);
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendOpenStart,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Started,
            None,
            None,
            None,
        );
        assert_eq!(instance_alias, "node.a");
        self.state.opens.fetch_add(1, Ordering::AcqRel);
        let backend: Box<dyn InputBackend> = Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        });
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::BackendOpenResult,
            ObservationOperation::Backend,
            ObservationThreadRole::Backend,
            ObservationOutcome::Success,
            None,
            None,
            None,
        );
        Ok(backend)
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        assert_eq!(instance_alias, "node.a");
        self.state.capture_opens.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }

    fn control_application(
        &self,
        instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        assert_eq!(instance_alias, "node.a");
        Ok(())
    }
}

impl ExecutionBackendProvider for NeutralProjectProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["instance-neutral".to_owned()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "instance-neutral")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "local-neutral-endpoint"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        self.state.opens.fetch_add(1, Ordering::AcqRel);
        Err(DeviceError::fatal("project interface opened input"))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        self.state.capture_opens.fetch_add(1, Ordering::AcqRel);
        Err(DeviceError::fatal("project interface opened capture"))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "project interface controlled application",
        ))
    }
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn host(root: &TempDir, state: Arc<FakeState>, lease_ttl_ms: u64) -> RuntimeHost {
    RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"runtime-client-test-salt")
            .with_governance_capability(TEST_GOVERNANCE_CAPABILITY)
            .with_io_timeout(Duration::from_millis(500))
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms,
                ..SchedulerConfig::default()
            }),
        Arc::new(FakeProvider {
            instance_id: instance_id(),
            state,
        }),
    )
    .expect("runtime host")
}

fn client(root: &TempDir) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("runtime client")
}

fn lab_client(root: &TempDir) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Lab, EventSource::Lab)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("Lab runtime client")
}

fn client_with_timeout(root: &TempDir, io_timeout: Duration) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(io_timeout),
    )
    .expect("runtime client")
}

#[cfg(feature = "test-observation")]
fn start_test_observation() -> (TestObservationCapture, HostTestObservationGuard) {
    let capture = TestObservationCapture::start().expect("start test observation");
    let recorder = capture.recorder();
    let host = actingcommand_runtime_host::test_observation::install(Arc::new(move |event| {
        record_host_observation(&recorder, event);
    }))
    .expect("install host test observation");
    (capture, host)
}

#[cfg(feature = "test-observation")]
fn record_host_observation(recorder: &TestObservationRecorder, event: HostTestObservation) {
    let stage = match event.point() {
        HostTestObservationPoint::FrameReceived => ObservationStage::HostFrameReceived,
        HostTestObservationPoint::DispatchStart => ObservationStage::HostDispatchStart,
        HostTestObservationPoint::DispatchResult => ObservationStage::HostDispatchResult,
        HostTestObservationPoint::ReceiptWriteStart => ObservationStage::HostReceiptWriteStart,
        HostTestObservationPoint::ReceiptWriteResult => ObservationStage::HostReceiptWriteResult,
        HostTestObservationPoint::ConnectionExit => ObservationStage::HostConnectionExit,
        HostTestObservationPoint::ConnectionCleanupResult => ObservationStage::HostCleanupResult,
    };
    let outcome = match event.outcome() {
        HostTestObservationOutcome::Started => ObservationOutcome::Started,
        HostTestObservationOutcome::Success => ObservationOutcome::Success,
        HostTestObservationOutcome::Error => ObservationOutcome::Failure,
        HostTestObservationOutcome::Closed => ObservationOutcome::Closed,
        HostTestObservationOutcome::Shutdown => ObservationOutcome::Shutdown,
    };
    let operation = match event.operation() {
        HostTestObservationOperation::AcquireLease => ObservationOperation::AcquireLease,
        HostTestObservationOperation::RenewLease => ObservationOperation::RenewLease,
        HostTestObservationOperation::Input => ObservationOperation::Input,
        HostTestObservationOperation::ReleaseLease => ObservationOperation::ReleaseLease,
        HostTestObservationOperation::Connection => ObservationOperation::Connection,
        HostTestObservationOperation::Other => ObservationOperation::Other,
    };
    let identities = event.identities();
    recorder.record_sanitized(
        stage,
        operation,
        ObservationThreadRole::Host,
        outcome,
        ObservationIdentities {
            request: identities.request(),
            correlation: identities.correlation(),
            instance: identities.instance(),
            lease: identities.lease(),
            holder: identities.holder(),
            token: identities.token(),
        },
    );
}

#[cfg(feature = "test-observation")]
#[derive(Clone, Copy)]
enum ExpectedObservationPath {
    HeartbeatProxy,
    LongInput,
}

#[cfg(feature = "test-observation")]
fn assert_test_observation_trace(records: &[ObservationRecord], expected: ExpectedObservationPath) {
    assert!(!records.is_empty());
    for (index, record) in records.iter().enumerate() {
        assert_eq!(
            record.sequence,
            u64::try_from(index).expect("trace index fits u64") + 1
        );
        assert!(!record.thread_name.is_empty());
    }
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].elapsed_micros <= pair[1].elapsed_micros)
    );

    for (start, result) in [
        (
            ObservationStage::ClientRequestWriteStart,
            ObservationStage::ClientRequestWriteResult,
        ),
        (
            ObservationStage::ReceiptHeaderWait,
            ObservationStage::ReceiptHeaderResult,
        ),
        (
            ObservationStage::ReceiptBodyWait,
            ObservationStage::ReceiptBodyResult,
        ),
        (
            ObservationStage::HostDispatchStart,
            ObservationStage::HostDispatchResult,
        ),
        (
            ObservationStage::HostReceiptWriteStart,
            ObservationStage::HostReceiptWriteResult,
        ),
        (
            ObservationStage::BackendOpenStart,
            ObservationStage::BackendOpenResult,
        ),
    ] {
        assert_before(records, start, result);
    }
    assert_before(
        records,
        ObservationStage::ClientRequestCreated,
        ObservationStage::HostFrameReceived,
    );
    assert_before(
        records,
        ObservationStage::HostReceiptWriteStart,
        ObservationStage::ReceiptHeaderResult,
    );
    assert_before(
        records,
        ObservationStage::ReceiptBodyResult,
        ObservationStage::ReceiptValidationResult,
    );
    assert_before(
        records,
        ObservationStage::ReceiptValidationResult,
        ObservationStage::ClientTerminalResult,
    );
    assert!(records.iter().any(|record| {
        record.stage == ObservationStage::HostConnectionExit
            && matches!(
                record.outcome,
                ObservationOutcome::Closed | ObservationOutcome::Shutdown
            )
    }));
    assert!(records.iter().any(|record| {
        record.stage == ObservationStage::HostCleanupResult
            && record.outcome == ObservationOutcome::Success
    }));
    assert!(records.iter().any(|record| {
        record.stage == ObservationStage::BackendClose
            && record.outcome == ObservationOutcome::Closed
    }));

    let client_requests = records
        .iter()
        .filter(|record| record.stage == ObservationStage::ClientRequestCreated)
        .map(|record| {
            (
                record
                    .identities
                    .request
                    .expect("client request has request ordinal"),
                record
                    .identities
                    .correlation
                    .expect("client request has correlation ordinal"),
            )
        })
        .collect::<Vec<_>>();
    assert!(!client_requests.is_empty());
    for (request, correlation) in client_requests {
        for stage in [
            ObservationStage::HostFrameReceived,
            ObservationStage::HostDispatchResult,
            ObservationStage::HostReceiptWriteResult,
            ObservationStage::ReceiptValidationResult,
        ] {
            assert!(records.iter().any(|record| {
                record.stage == stage
                    && record.identities.request == Some(request)
                    && record.identities.correlation == Some(correlation)
            }));
        }
    }

    for identity in [
        |record: &ObservationRecord| record.identities.instance,
        |record: &ObservationRecord| record.identities.lease,
        |record: &ObservationRecord| record.identities.holder,
    ] {
        let ordinals = records
            .iter()
            .filter(|record| {
                matches!(
                    record.operation,
                    ObservationOperation::AcquireLease
                        | ObservationOperation::RenewLease
                        | ObservationOperation::Input
                        | ObservationOperation::ReleaseLease
                )
            })
            .filter_map(identity)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            ordinals.len(),
            1,
            "lease-chain identity split: {ordinals:?}"
        );
    }

    let token_lineage = records
        .iter()
        .filter_map(|record| record.identities.token)
        .collect::<BTreeSet<_>>();
    match expected {
        ExpectedObservationPath::HeartbeatProxy => {
            assert_before(
                records,
                ObservationStage::ProxyAcquireStart,
                ObservationStage::ProxyAcquireResult,
            );
            assert!(records.iter().any(|record| {
                record.stage == ObservationStage::HeartbeatWaitResult
                    && record.outcome == ObservationOutcome::Timeout
            }));
            assert_before(
                records,
                ObservationStage::HeartbeatRenewStart,
                ObservationStage::HeartbeatRenewResult,
            );
            assert_before(
                records,
                ObservationStage::ProxyInputStart,
                ObservationStage::ProxyInputResult,
            );
            assert_before(
                records,
                ObservationStage::BackendInputStart,
                ObservationStage::BackendInputResult,
            );
            assert_before(
                records,
                ObservationStage::ProxyStopSend,
                ObservationStage::HeartbeatJoinStart,
            );
            assert_before(
                records,
                ObservationStage::HeartbeatJoinStart,
                ObservationStage::HeartbeatJoinResult,
            );
            assert_before(
                records,
                ObservationStage::HeartbeatJoinResult,
                ObservationStage::ProxyReleaseStart,
            );
            assert_before(
                records,
                ObservationStage::ProxyReleaseStart,
                ObservationStage::ProxyReleaseResult,
            );
            assert!(token_lineage.len() >= 2, "renewal token lineage missing");
        }
        ExpectedObservationPath::LongInput => {
            assert_before(
                records,
                ObservationStage::ClientAcquireStart,
                ObservationStage::ClientAcquireResult,
            );
            assert_before(
                records,
                ObservationStage::ClientInputStart,
                ObservationStage::BackendLongInputStart,
            );
            assert_before(
                records,
                ObservationStage::BackendLongInputStart,
                ObservationStage::BackendLongInputResult,
            );
            assert_before(
                records,
                ObservationStage::BackendLongInputResult,
                ObservationStage::ClientInputResult,
            );
            assert_before(
                records,
                ObservationStage::ClientReleaseStart,
                ObservationStage::ClientReleaseResult,
            );
            assert_eq!(token_lineage.len(), 1, "unexpected token replacement");
        }
    }
}

#[cfg(feature = "test-observation")]
fn assert_before(records: &[ObservationRecord], first: ObservationStage, second: ObservationStage) {
    let first = records
        .iter()
        .position(|record| record.stage == first)
        .unwrap_or_else(|| panic!("missing observation stage: {first:?}"));
    let second = records
        .iter()
        .position(|record| record.stage == second)
        .unwrap_or_else(|| panic!("missing observation stage: {second:?}"));
    assert!(
        first < second,
        "observation order reversed: {first} >= {second}"
    );
}

fn scripted_runtime(
    root: &TempDir,
    script: impl FnOnce(TcpListener, OwnerEpoch) + Send + 'static,
) -> thread::JoinHandle<()> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind scripted runtime");
    let address = listener.local_addr().expect("scripted runtime address");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let owner_epoch = *ids.mint_owner_epoch().expect("owner epoch").transport();
    let info = RuntimeInfo::new(
        std::process::id(),
        address.ip().to_string(),
        address.port(),
        owner_epoch,
        1,
    )
    .expect("scripted runtime info");
    fs::write(
        root.path().join(RUNTIME_INFO_FILE),
        serde_json::to_vec(&info).expect("encode scripted runtime info"),
    )
    .expect("write scripted runtime info");
    thread::spawn(move || script(listener, owner_epoch))
}

fn read_scripted_request(stream: &mut TcpStream) -> RuntimeRequest {
    read_scripted_request_at(stream, "scripted")
}

fn read_scripted_request_at(stream: &mut TcpStream, stage: &str) -> RuntimeRequest {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .unwrap_or_else(|error| panic!("{stage} request header: {error}"));
    let length = u32::from_be_bytes(header) as usize;
    assert!(length > 0 && length <= DEFAULT_RUNTIME_MAX_FRAME_BYTES);
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .unwrap_or_else(|error| panic!("{stage} request body: {error}"));
    serde_json::from_slice(&body).expect("decode request")
}

fn write_scripted_result(stream: &mut TcpStream, request: &RuntimeRequest, result: RuntimeResult) {
    let receipt = RuntimeReceipt::success(request, RuntimeReceiptState::Completed, None, result)
        .expect("scripted receipt");
    let body = serde_json::to_vec(&receipt).expect("encode scripted receipt");
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .expect("receipt header");
    stream.write_all(&body).expect("receipt body");
    stream.flush().expect("flush receipt");
}

fn release_capture_after_io_budget(
    gate: Arc<CaptureGate>,
    io_timeout: Duration,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        gate.entered.wait();
        thread::sleep(io_timeout + Duration::from_millis(100));
        gate.release.wait();
    })
}

fn respond_to_health(stream: &mut TcpStream, owner_epoch: OwnerEpoch) {
    let request = read_scripted_request(stream);
    assert!(matches!(request.operation(), RuntimeOperation::Health));
    write_scripted_result(stream, &request, RuntimeResult::Health { owner_epoch });
}

fn respond_with_empty_event_page(stream: &mut TcpStream) {
    let request = read_scripted_request(stream);
    let RuntimeOperation::QueryEvents { page, .. } = request.operation() else {
        panic!("expected event query after flow receipt")
    };
    let page = RuntimeEventQueryPage::new(Vec::new(), 0, page.limit(), false, None)
        .expect("empty event page");
    write_scripted_result(stream, &request, RuntimeResult::EventPage { page });
}

fn contained_task_request() -> ContainedTaskRequest {
    ContainedTaskRequest::new("C:\\fixture\\contained-task.zip", "0".repeat(64))
        .expect("contained task request")
}

fn receipt_eof_error(deadline: Instant) -> RuntimeClientError {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind EOF runtime");
    let address = listener.local_addr().expect("EOF runtime address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept EOF client");
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).expect("request header");
        let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut body).expect("request body");
    });
    let mut stream = TcpStream::connect(address).expect("connect EOF runtime");
    let error = exchange::<_, serde_json::Value>(
        &mut stream,
        &"request",
        DEFAULT_RUNTIME_MAX_FRAME_BYTES,
        Some(ReceiptReadDeadline::at(deadline, "runtime_receipt_timeout")),
        None,
    )
    .expect_err("peer EOF must fail the receipt exchange");
    server.join().expect("EOF runtime");
    error
}

#[test]
fn receipt_eof_at_or_after_deadline_is_typed_timeout() {
    let deadline = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("past deadline");
    assert_eq!(
        receipt_eof_error(deadline).code(),
        "runtime_receipt_timeout"
    );
}

#[test]
fn receipt_eof_before_deadline_remains_connection_failure() {
    let deadline = Instant::now() + Duration::from_secs(30);
    assert_eq!(
        receipt_eof_error(deadline).code(),
        "runtime_receipt_header_failed"
    );
}

#[test]
fn contained_task_can_outlive_the_general_five_second_exchange_timeout() {
    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, |listener, owner_epoch| {
        let (mut stream, _) = listener.accept().expect("accept client");
        respond_to_health(&mut stream, owner_epoch);

        let request = read_scripted_request(&mut stream);
        assert!(matches!(
            request.operation(),
            RuntimeOperation::RunContainedTask { request, .. }
                if request.response_deadline_ms()
                    == ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS
        ));
        thread::sleep(Duration::from_millis(5_100));
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        write_scripted_result(
            &mut stream,
            &request,
            RuntimeResult::ContainedTaskCompleted {
                run_id: *ids.mint_run_id().expect("run id").transport(),
                task_id: *ids.mint_task_id().expect("task id").transport(),
                task_request_id: request.request_id(),
                response_deadline_monotonic_ms: Some(60_000),
                outcome: TaskOutcome::Success,
                final_page: None,
                executed_steps: 1,
            },
        );
        respond_with_empty_event_page(&mut stream);
    });
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("runtime client");

    let started = std::time::Instant::now();
    let output = client
        .run_contained_task("node.a", contained_task_request())
        .expect("task inside configured response deadline");
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ContainedTaskCompleted { .. })
    ));
    server.join().expect("scripted runtime");
}

#[test]
fn successful_contained_task_returns_complete_ordered_projection_across_event_pages() {
    let root = TempDir::new().expect("tempdir");
    let event_count = usize::from(MAX_RUNTIME_EVENT_QUERY_EVENTS) + 17;
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        let (mut stream, _) = listener.accept().expect("accept client");
        respond_to_health(&mut stream, owner_epoch);

        let task_request = read_scripted_request(&mut stream);
        assert!(matches!(
            task_request.operation(),
            RuntimeOperation::RunContainedTask { .. }
        ));
        let correlation_id = task_request.correlation_id();
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        write_scripted_result(
            &mut stream,
            &task_request,
            RuntimeResult::ContainedTaskCompleted {
                run_id: *ids.mint_run_id().expect("run id").transport(),
                task_id: *ids.mint_task_id().expect("task id").transport(),
                task_request_id: task_request.request_id(),
                response_deadline_monotonic_ms: Some(60_000),
                outcome: TaskOutcome::Success,
                final_page: Some("fixture/final".to_owned()),
                executed_steps: 1,
            },
        );

        let events = (1..=u64::try_from(event_count).expect("event count"))
            .map(|sequence| projected_task_event(&ids, sequence))
            .collect::<Vec<_>>();
        let snapshot = u64::try_from(events.len()).expect("snapshot position");
        let mut offset = 0_usize;
        while offset < events.len() {
            let query_request = read_scripted_request(&mut stream);
            let RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } = query_request.operation()
            else {
                panic!("expected event query after task terminal")
            };
            assert_eq!(query.correlation_id, Some(correlation_id));
            assert_eq!(*profile, ProjectionProfile::Forensic);
            assert_eq!(
                page.cursor().map(RuntimeEventQueryCursor::after_sequence),
                (offset > 0).then_some(u64::try_from(offset).expect("cursor offset"))
            );
            let end = offset
                .checked_add(usize::from(page.limit()))
                .expect("page end")
                .min(events.len());
            let has_more = end < events.len();
            let next_cursor = has_more.then(|| {
                RuntimeEventQueryCursor::new(
                    snapshot,
                    u64::try_from(end).expect("cursor sequence"),
                    query,
                    *profile,
                )
                .expect("event cursor")
            });
            let page = RuntimeEventQueryPage::new(
                events[offset..end].to_vec(),
                snapshot,
                page.limit(),
                has_more,
                next_cursor,
            )
            .expect("event page");
            write_scripted_result(
                &mut stream,
                &query_request,
                RuntimeResult::EventPage { page },
            );
            offset = end;
        }
    });
    let client = client(&root);

    let output = client
        .run_contained_task("node.a", contained_task_request())
        .expect("successful task projection");
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            ..
        })
    ));
    assert_eq!(output.events().len(), event_count);
    assert_eq!(
        output
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=u64::try_from(event_count).expect("event count")).collect::<Vec<_>>()
    );
    drop(client);
    server.join().expect("scripted runtime");
}

#[test]
fn contained_task_response_timeout_is_typed_and_runs_reset_recovery() {
    let root = TempDir::new().expect("tempdir");
    let recovered = Arc::new(AtomicBool::new(false));
    let server_recovered = Arc::clone(&recovered);
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        let (mut timed_out_stream, _) = listener.accept().expect("accept timed task client");
        respond_to_health(&mut timed_out_stream, owner_epoch);
        let request = read_scripted_request(&mut timed_out_stream);
        assert!(matches!(
            request.operation(),
            RuntimeOperation::RunContainedTask { request, .. }
                if request.response_deadline_ms() == 100
        ));
        let (mut recovery_stream, _) = listener.accept().expect("accept recovery client");
        drop(timed_out_stream);
        recovery_stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bounded recovery script");
        respond_to_health(&mut recovery_stream, owner_epoch);
        let cancel = read_scripted_request_at(&mut recovery_stream, "cancellation");
        assert!(matches!(
            cancel.operation(),
            RuntimeOperation::CancelContainedTask { task_request_id }
                if task_request_id == &request.request_id()
        ));
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        write_scripted_result(
            &mut recovery_stream,
            &cancel,
            RuntimeResult::ContainedTaskCancellation {
                task_request_id: request.request_id(),
                status: actingcommand_contract::ContainedTaskCancellationStatus::Terminal {
                    deadline_monotonic_ms: Some(100),
                    outcome: TaskOutcome::Cancelled,
                    reason: Some(
                        actingcommand_contract::ContainedTaskCancellationReason::DeadlineExceeded,
                    ),
                    task_terminal: TerminalEvent {
                        sequence: 1,
                        event_id: *ids.mint_event_id().expect("task terminal id").transport(),
                    },
                    lease_terminal: TerminalEvent {
                        sequence: 2,
                        event_id: *ids.mint_event_id().expect("lease terminal id").transport(),
                    },
                    lease_disposition: actingcommand_contract::ContainedTaskLeaseTerminal::Released,
                },
            },
        );
        let reset = read_scripted_request_at(&mut recovery_stream, "safe reset");
        assert!(matches!(
            reset.operation(),
            RuntimeOperation::SafeReset { .. }
        ));
        write_scripted_result(
            &mut recovery_stream,
            &reset,
            RuntimeResult::SafeResetCompleted {
                action_id: *ids.mint_action_id().expect("action id").transport(),
            },
        );
        respond_with_empty_event_page(&mut recovery_stream);
        server_recovered.store(true, Ordering::Release);
    });
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(50))
            .with_backend_open_timeout(Duration::from_millis(100)),
    )
    .expect("runtime client");

    let error = client
        .run_contained_task(
            "node.a",
            contained_task_request()
                .with_response_deadline_ms(100)
                .expect("bounded task deadline"),
        )
        .expect_err("missing task receipt must hit the bounded response deadline");
    assert_eq!(
        error.code(),
        "runtime_contained_task_response_timeout",
        "{error:#?}"
    );
    assert!(error.is_fatal());
    server
        .join()
        .unwrap_or_else(|_| panic!("scripted runtime failed after {error:#?}"));
    assert!(recovered.load(Ordering::Acquire));
}

#[test]
fn contained_task_pre_deadline_eof_remains_receipt_failure() {
    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, |listener, owner_epoch| {
        let (mut stream, _) = listener.accept().expect("accept task client");
        respond_to_health(&mut stream, owner_epoch);
        let request = read_scripted_request(&mut stream);
        assert!(matches!(
            request.operation(),
            RuntimeOperation::RunContainedTask { request, .. }
                if request.response_deadline_ms() == 1_000
        ));
    });
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(50)),
    )
    .expect("runtime client");

    let error = client
        .run_contained_task(
            "node.a",
            contained_task_request()
                .with_response_deadline_ms(1_000)
                .expect("bounded task deadline"),
        )
        .expect_err("pre-deadline EOF must remain a connection failure");
    assert_eq!(error.code(), "runtime_receipt_header_failed", "{error:#?}");
    server.join().expect("scripted runtime");
}

#[test]
fn project_interface_is_consistent_across_clients_and_read_only() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"project-interface-test-salt")
            .with_io_timeout(Duration::from_millis(500)),
        Arc::new(NeutralProjectProvider {
            instance_id: instance_id(),
            state: Arc::clone(&state),
        }),
    )
    .expect("runtime host");
    let clients = [
        (EventActor::Cli, EventSource::Cli),
        (EventActor::Ui, EventSource::Ui),
        (EventActor::Agent, EventSource::Adapter),
    ]
    .map(|(actor, source)| {
        RuntimeProjectClient::connect(
            RuntimeClientConfig::new(root.path(), actor, source)
                .with_io_timeout(Duration::from_millis(500)),
        )
        .expect("project client")
    });
    let snapshots = clients
        .iter()
        .map(|client| client.snapshot().expect("project snapshot"))
        .collect::<Vec<_>>();
    assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
    let status = clients[0].status().expect("runtime status");
    assert_eq!(status.instances()[0].instance_alias(), "instance-neutral");
    assert!(snapshots[0].project.is_none());
    assert!(snapshots[0].catalog.is_none());
    for version in [
        actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V2,
        actingcommand_contract::PROJECT_INTERFACE_CONTRACT_V1,
    ] {
        let snapshot = clients[0]
            .snapshot_with_versions(vec![version.to_owned()])
            .expect("legacy project snapshot");
        assert_eq!(snapshot.ledger_position, snapshots[0].ledger_position);
        assert!(!snapshot.decision_page.has_more());
    }
    assert_eq!(state.opens.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 0);

    let error = clients[0]
        .snapshot_with_versions(vec!["actingcommand.project-interface.v9".to_owned()])
        .expect_err("unknown contract version must fail loud");
    assert_eq!(error.code(), "runtime_request_rejected");
    assert_eq!(
        error.projection().expect("typed rejection").code,
        RuntimeErrorCode::ProtocolInvalid
    );
    assert!(!error.is_fatal());
    drop(host);
}

#[test]
fn typed_client_discovers_runtime_and_routes_queries_and_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    assert_eq!(
        client.health().expect("health"),
        host.runtime_info().owner_epoch()
    );
    let status = client.status().expect("status");
    assert_eq!(status.owner_epoch(), host.runtime_info().owner_epoch());
    assert_eq!(status.instances().len(), 1);
    assert_eq!(status.instances()[0].instance_alias(), "node.a");
    assert!(!status.instances()[0].lease_active());
    assert!(
        client.monitor_status().expect("monitor status").instances()[0]
            .policy()
            .is_none()
    );
    let monitor_policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("monitor policy");
    assert_eq!(
        client
            .configure_monitor("node.a", monitor_policy.clone())
            .expect("configure monitor")
            .policy(),
        Some(&monitor_policy)
    );
    assert_eq!(
        client
            .monitor_status()
            .expect("configured monitor status")
            .instances()[0]
            .policy(),
        Some(&monitor_policy)
    );
    assert!(
        client
            .clear_monitor("node.a")
            .expect("clear monitor")
            .policy()
            .is_none()
    );
    let token = client.acquire_lease("node.a").expect("lease");
    assert!(client.status().expect("leased status").instances()[0].lease_active());
    client
        .input(&token, InputAction::Tap { x: 10, y: 20 })
        .expect("input");
    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("events");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::InputCommitted)
    );
    client.release_lease(&token).expect("release");
    assert!(!client.status().expect("released status").instances()[0].lease_active());
    assert_eq!(state.opens.load(Ordering::Acquire), 1);
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn typed_client_records_client_actions_and_approval_decisions_through_runtime() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::User, EventSource::Ui)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("governance runtime client");

    client
        .record_client_action(
            ClientActionRecord::new(
                "overview",
                "refresh",
                ClientActionKind::Button,
                Some("node.a".to_owned()),
                None,
            )
            .expect("client action"),
        )
        .expect("record client action");
    client
        .authenticate_governance(TEST_GOVERNANCE_CAPABILITY)
        .expect("authenticate governance");
    client
        .record_approval_decision(
            ApprovalDecisionRecord::new(
                "approval:client-fixture",
                ApprovalDisposition::Approved,
                ApprovalTarget::Catalog {
                    catalog_hash: format!("sha256:{}", "a".repeat(64)),
                    catalog_version: 1,
                },
                "user_confirmed",
            )
            .expect("approval decision"),
        )
        .expect("record approval decision");

    let events = client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ClientAction)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ApprovalDecision)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn subscription_waits_for_new_events_and_returns_a_resumable_batch() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let subscriber = client(&root);
    let after_sequence = subscriber
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("initial events")
        .last()
        .map_or(0, |event| event.sequence);
    let producer = client(&root);
    let producer_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        let token = producer.acquire_lease("node.a").expect("producer lease");
        producer.release_lease(&token).expect("producer release");
    });

    let batch = subscriber
        .subscribe_events(
            RuntimeSubscriptionRequest::new(
                EventQuery::default(),
                ProjectionProfile::Forensic,
                SubscriptionCursor { after_sequence },
                500,
                32,
            )
            .expect("subscription request"),
        )
        .expect("subscription batch");
    producer_thread.join().expect("producer thread");

    assert!(!batch.timed_out());
    assert!(!batch.events().is_empty());
    assert!(
        batch
            .events()
            .iter()
            .all(|event| event.sequence > after_sequence)
    );
    assert_eq!(
        batch.next_cursor().after_sequence,
        batch.events().last().expect("last event").sequence
    );
    drop(subscriber);
    host.close().expect("close host");
}

#[test]
fn subscription_timeout_is_an_explicit_idle_batch() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let subscriber = client(&root);
    let correlation_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_correlation_id()
        .expect("correlation")
        .transport();
    let after_sequence = subscriber
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("initial events")
        .last()
        .map_or(0, |event| event.sequence);

    let batch = subscriber
        .subscribe_events(
            RuntimeSubscriptionRequest::new(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
                SubscriptionCursor { after_sequence },
                20,
                8,
            )
            .expect("subscription request"),
        )
        .expect("timeout batch");

    assert!(batch.timed_out());
    assert!(batch.events().is_empty());
    assert_eq!(batch.next_cursor().after_sequence, after_sequence);
    drop(subscriber);
    host.close().expect("close host");
}

#[test]
fn authoring_session_reuses_one_runtime_correlation_and_requires_durable_terminals() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = lab_client(&root);
    let session = client.begin_authoring_session().expect("authoring session");
    let expected = [
        (ResourceAuthoringPhase::AuthoringStarted, None),
        (ResourceAuthoringPhase::DraftBuilt, None),
        (ResourceAuthoringPhase::ValidationCompleted, None),
        (ResourceAuthoringPhase::PromoteIntent, None),
        (ResourceAuthoringPhase::Promoted, None),
    ];
    let mut previous_sequence = 0;
    for (phase, failure_code) in expected {
        let terminal = session
            .append(
                ResourceAuthoringEvent::new(
                    phase,
                    "draft-a",
                    "resource-root",
                    "b".repeat(64),
                    vec!["operations/task-a/task.json".to_string()],
                    failure_code,
                )
                .expect("authoring event"),
            )
            .expect("durable authoring event");
        assert!(terminal.sequence > previous_sequence);
        previous_sequence = terminal.sequence;
    }

    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("authoring events");
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::ResourceAuthoringStarted,
            EventType::ResourceDraftBuilt,
            EventType::ResourceValidationCompleted,
            EventType::ResourcePromoteIntent,
            EventType::ResourcePromoted,
        ]
    );
    assert!(
        events.iter().all(|event| {
            event.links.correlation_id().copied() == Some(session.correlation_id())
        })
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn non_lab_client_cannot_open_authoring_session() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = client(&root);
    let error = match client.begin_authoring_session() {
        Ok(_) => panic!("CLI authoring session must fail"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "runtime_authoring_origin_invalid");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn debug_session_correlates_runtime_capture_scheduler_input_and_release() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = lab_client(&root);
    let session = client.begin_debug_session().expect("debug session");

    let observation = session
        .observe_readonly("node.a")
        .expect("debug observation");
    assert!(matches!(
        observation.result(),
        Some(RuntimeResult::ReadonlyObservationCompleted { .. })
    ));
    let token = session.acquire_lease("node.a").expect("debug lease");
    session
        .input(&token, InputAction::Tap { x: 10, y: 20 })
        .expect("debug input");
    session.release_lease(&token).expect("debug release");

    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("debug events");
    assert!(
        events
            .iter()
            .all(|event| event.links.correlation_id().copied() == Some(session.correlation_id()))
    );
    for event_type in [
        EventType::CaptureCompleted,
        EventType::LeaseGranted,
        EventType::InputCommitted,
        EventType::LeaseReleased,
    ] {
        assert!(events.iter().any(|event| event.event_type == event_type));
    }
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn lab_run_debug_event_requires_a_verified_package_context() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = lab_client(&root);
    let session = client.begin_debug_session().expect("debug session");

    let error = session
        .record_event(RuntimeDebugEvent::requested(RuntimeDebugOperation::LabRun))
        .expect_err("Lab run must be admitted through debug-package containment");

    assert_eq!(error.code(), "runtime_request_rejected");
    assert_eq!(
        error.projection().expect("Runtime rejection").code,
        RuntimeErrorCode::InvalidRequest
    );
    assert!(
        session
            .query_events(ProjectionProfile::Forensic)
            .expect("debug events")
            .iter()
            .all(|event| event.event_type != EventType::LabRequest)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn non_lab_client_cannot_open_debug_session() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = client(&root);

    let error = client
        .begin_debug_session()
        .expect_err("CLI debug session must fail");

    assert_eq!(error.code(), "runtime_debug_origin_invalid");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn typed_client_queues_polls_and_cancels_connection_bound_leases() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let first = client(&root);
    let second = client(&root);
    let first_token = first.acquire_lease("node.a").expect("first lease");
    let queued = second
        .queue_lease(
            "node.a",
            LeaseQueuePolicy::new(LeasePriority::Normal, 1_000).expect("queue policy"),
        )
        .expect("queue lease");
    let LeaseAdmission::Queued(status) = queued else {
        panic!("expected queued admission");
    };
    assert!(matches!(
        second
            .poll_queued_lease(status.request_id())
            .expect("poll pending"),
        LeaseAdmission::Queued(_)
    ));
    first.release_lease(&first_token).expect("release first");
    let LeaseAdmission::Granted(second_token) = second
        .poll_queued_lease(status.request_id())
        .expect("poll granted")
    else {
        panic!("expected transferred lease");
    };
    second.release_lease(&second_token).expect("release second");

    let third = first.acquire_lease("node.a").expect("third lease");
    let queued = second
        .queue_lease(
            "node.a",
            LeaseQueuePolicy::new(LeasePriority::Normal, 1_000).expect("queue policy"),
        )
        .expect("queue lease");
    let LeaseAdmission::Queued(status) = queued else {
        panic!("expected queued admission");
    };
    second
        .cancel_queued_lease(status.request_id())
        .expect("cancel queue");
    first.release_lease(&third).expect("release third");
    assert_eq!(state.opens.load(Ordering::Acquire), 0);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn readonly_observation_returns_host_receipt_and_correlated_projection() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let output = client
        .observe_readonly("node.a")
        .expect("readonly observation");

    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ReadonlyObservationCompleted { observation })
            if observation.width() == 2
                && observation.height() == 1
                && observation.verdict() == actingcommand_contract::RecognitionVerdict::FrameDecoded
                && observation.capture_backend() == RuntimeCaptureBackend::AdbScreencap
                && observation.artifact().object_key().is_some()
    ));
    assert_eq!(
        output
            .events()
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::SchedulerAdmitted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
        ]
    );
    assert_eq!(state.opens.load(Ordering::Acquire), 0);
    assert_eq!(state.inputs.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
}

#[test]
fn observe_readonly_preserves_delayed_host_capture_failure_beyond_io_timeout() {
    let root = TempDir::new().expect("tempdir");
    let io_timeout = Duration::from_millis(100);
    let gate = Arc::new(CaptureGate::new());
    let state = Arc::new(FakeState {
        fail_capture: AtomicBool::new(true),
        capture_gate: Some(Arc::clone(&gate)),
        ..FakeState::default()
    });
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(io_timeout)
            .with_backend_open_timeout(Duration::from_secs(2)),
    )
    .expect("runtime client");
    let releaser = release_capture_after_io_budget(gate, io_timeout);

    let result = client.observe_readonly("node.a");
    releaser.join().expect("release delayed capture");
    let error = result.expect_err("Host capture failure must remain typed");

    assert_eq!(error.code(), "runtime_request_rejected", "{error:#?}");
    assert_eq!(
        error.projection().expect("typed Host failure").code,
        RuntimeErrorCode::CaptureFailed
    );
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn debug_observe_readonly_preserves_one_delayed_success_chain_beyond_io_timeout() {
    let root = TempDir::new().expect("tempdir");
    let io_timeout = Duration::from_millis(100);
    let gate = Arc::new(CaptureGate::new());
    let state = Arc::new(FakeState {
        capture_gate: Some(Arc::clone(&gate)),
        ..FakeState::default()
    });
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Lab, EventSource::Lab)
            .with_io_timeout(io_timeout)
            .with_backend_open_timeout(Duration::from_secs(2)),
    )
    .expect("Lab runtime client");
    let session = client.begin_debug_session().expect("debug session");
    let releaser = release_capture_after_io_budget(gate, io_timeout);

    let result = session.observe_readonly("node.a");
    releaser.join().expect("release delayed capture");
    let receipt = result.expect("delayed Host observation receipt");

    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ReadonlyObservationCompleted { .. })
    ));
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    let events = session
        .query_events(ProjectionProfile::Forensic)
        .expect("debug observation events");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::CaptureCompleted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::RecognitionCompleted)
            .count(),
        1
    );
    drop(session);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
}

#[test]
fn observe_readonly_connection_close_before_receipt_remains_transport_failure() {
    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, |listener, owner_epoch| {
        let (mut stream, _) = listener.accept().expect("accept client");
        respond_to_health(&mut stream, owner_epoch);
        let request = read_scripted_request(&mut stream);
        assert!(matches!(
            request.operation(),
            RuntimeOperation::ObserveReadonly { .. }
        ));
    });
    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(100))
            .with_backend_open_timeout(Duration::from_secs(2)),
    )
    .expect("runtime client");

    let error = client
        .observe_readonly("node.a")
        .expect_err("missing Host receipt must remain a transport failure");

    assert_eq!(error.code(), "runtime_receipt_header_failed", "{error:#?}");
    server.join().expect("scripted runtime");
}

#[test]
fn receipt_timeout_selector_preserves_existing_operation_budgets() {
    let io_timeout = Duration::from_millis(100);
    let backend_open_timeout = Duration::from_secs(2);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let holder_id = *ids.mint_holder_id().expect("holder id").transport();

    assert_eq!(
        receipt_response_timeout(
            &RuntimeOperation::ObserveReadonly {
                instance_alias: "node.a".to_string(),
            },
            io_timeout,
            backend_open_timeout,
        ),
        backend_open_timeout
    );
    assert_eq!(
        receipt_response_timeout(
            &RuntimeOperation::AcquireLease {
                instance_alias: "node.a".to_string(),
                holder_id,
            },
            io_timeout,
            backend_open_timeout,
        ),
        backend_open_timeout
    );
    assert_eq!(
        receipt_response_timeout(
            &RuntimeOperation::SafeReset {
                instance_alias: "node.a".to_string(),
                holder_id,
            },
            io_timeout,
            backend_open_timeout,
        ),
        backend_open_timeout
    );
    assert_eq!(
        receipt_response_timeout(&RuntimeOperation::Health, io_timeout, backend_open_timeout),
        io_timeout
    );
}

#[test]
fn capture_sequence_client_returns_exact_artifact_backed_frames_without_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let output = client
        .capture_sequence(
            "node.a",
            CaptureSequenceSpec::new(3, 1).expect("sequence spec"),
        )
        .expect("capture sequence");

    let sequence = match output.receipt().result() {
        Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence,
        other => panic!("unexpected capture sequence result: {other:?}"),
    };
    assert_eq!(sequence.observations().len(), 3);
    assert!(
        sequence
            .observations()
            .iter()
            .all(|observation| observation.artifact().object_key().is_some())
    );
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 3);
    assert_eq!(state.opens.load(Ordering::Acquire), 0);
    assert_eq!(state.inputs.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
}

#[test]
fn capture_failure_is_reported_to_runtime_and_never_returns_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let error = client
        .observe_readonly("node.a")
        .expect_err("capture failure must remain visible");

    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
    assert_eq!(
        error.projection().expect("runtime projection").code,
        RuntimeErrorCode::CaptureFailed
    );
    assert!(!error.is_fallback_eligible());
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn capture_failure_latches_the_daemon_session_without_retry_or_fallback() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);
    client
        .observe_readonly("node.a")
        .expect_err("first capture must fail");
    state.fail_capture.store(false, Ordering::Release);

    let second = client
        .observe_readonly("node.a")
        .expect_err("latched session must not reopen");

    assert!(second.is_fatal());
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_closes.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn malformed_daemon_frame_is_rejected_without_observation_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.invalid_capture.store(true, Ordering::Release);
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let error = client
        .observe_readonly("node.a")
        .expect_err("invalid frame must remain visible");

    assert_eq!(
        error.projection().expect("runtime projection").code,
        RuntimeErrorCode::CaptureFailed
    );
    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn safe_reset_uses_one_runtime_request_and_returns_ledger_projection() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let output = client.safe_reset("node.a").expect("safe reset");

    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::SafeResetCompleted { .. })
    ));
    assert_eq!(state.opens.load(Ordering::Acquire), 1);
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    assert_eq!(
        output.events().last().map(|event| event.event_type),
        Some(EventType::LeaseReleased)
    );
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn safe_reset_backend_failure_is_visible_and_releases_authority() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_input.store(true, Ordering::Release);
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let error = client
        .safe_reset("node.a")
        .expect_err("reset backend failure must be visible");

    assert_eq!(
        error.projection().expect("runtime projection").code,
        RuntimeErrorCode::BackendOperationFailed
    );
    assert_eq!(state.opens.load(Ordering::Acquire), 1);
    assert_eq!(state.inputs.load(Ordering::Acquire), 0);
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_input_proxy_renews_before_short_lease_expiry() {
    #[cfg(feature = "test-observation")]
    let (observation, host_observation) = start_test_observation();
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::for_test_observation());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);
    let mut proxy = RuntimeInputProxy::connect_with_heartbeat(
        client.clone(),
        "node.a",
        Duration::from_millis(50),
    )
    .expect("runtime input proxy");

    thread::sleep(Duration::from_millis(1_300));
    proxy
        .input(InputAction::Tap { x: 30, y: 40 })
        .expect("input after renewals");
    proxy.close().expect("close proxy");
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
    #[cfg(feature = "test-observation")]
    {
        drop(host_observation);
        let records = observation.finish().expect("seal test observation");
        assert_test_observation_trace(&records, ExpectedObservationPath::HeartbeatProxy);
    }
}

#[test]
fn dropping_runtime_input_proxy_releases_authority_but_keeps_the_daemon_session() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);
    let proxy = RuntimeInputProxy::connect_with_heartbeat(
        client.clone(),
        "node.a",
        Duration::from_millis(20),
    )
    .expect("runtime input proxy");

    drop(proxy);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    let replacement = client.acquire_lease("node.a").expect("replacement lease");
    client
        .release_lease(&replacement)
        .expect("replacement release");
    assert_eq!(state.opens.load(Ordering::Acquire), 0);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn long_input_extends_only_its_response_wait() {
    #[cfg(feature = "test-observation")]
    let (observation, host_observation) = start_test_observation();
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::for_test_observation());
    let host = host(&root, Arc::clone(&state), 5_000);
    let client = client_with_timeout(&root, Duration::from_millis(1_000));
    let token = client.acquire_lease("node.a").expect("lease");

    client
        .input(
            &token,
            InputAction::LongTap {
                x: 10,
                y: 20,
                duration_ms: 1_500,
            },
        )
        .expect("long input");

    client.release_lease(&token).expect("release");
    drop(client);
    host.close().expect("close host");
    #[cfg(feature = "test-observation")]
    {
        drop(host_observation);
        let records = observation.finish().expect("seal test observation");
        assert_test_observation_trace(&records, ExpectedObservationPath::LongInput);
    }
}

#[test]
fn missing_runtime_info_is_a_visible_fatal_error() {
    let root = TempDir::new().expect("tempdir");
    let error = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect_err("missing discovery must fail");
    assert_eq!(error.code(), "runtime_info_unavailable");
    assert!(error.is_fatal());
}

#[test]
fn broken_ipc_connection_latches_without_reconnect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, state, 1_000);
    let client = client(&root);
    host.close().expect("close host");

    let first = client.health().expect_err("closed runtime must fail");
    let second = client
        .health()
        .expect_err("terminal failure must be stable");
    assert_eq!(first, second);
    assert!(first.is_fatal());
}

#[test]
fn fallback_eligibility_is_narrower_than_runtime_host_fatality() {
    for code in [
        RuntimeErrorCode::LeaseBusy,
        RuntimeErrorCode::LeaseCooldown,
        RuntimeErrorCode::BackendOpenFailed,
        RuntimeErrorCode::BackendOperationFailed,
    ] {
        let error = RuntimeClientError::rejected(
            "test_runtime_error",
            RuntimeErrorProjection::new(code, false),
        );
        assert!(error.is_fallback_eligible());
    }

    for code in [
        RuntimeErrorCode::InvalidRequest,
        RuntimeErrorCode::RuntimeUnavailable,
        RuntimeErrorCode::RuntimeFatal,
        RuntimeErrorCode::OwnerConflict,
        RuntimeErrorCode::ProtocolInvalid,
        RuntimeErrorCode::InstanceUnknown,
        RuntimeErrorCode::LeaseExpired,
        RuntimeErrorCode::LeaseMissing,
        RuntimeErrorCode::StaleOwnerEpoch,
        RuntimeErrorCode::LeaseMismatch,
        RuntimeErrorCode::InstanceMismatch,
        RuntimeErrorCode::HolderMismatch,
        RuntimeErrorCode::ConnectionMismatch,
        RuntimeErrorCode::ReadonlyCapabilityInvalid,
        RuntimeErrorCode::CaptureFailed,
        RuntimeErrorCode::RecognitionFailed,
        RuntimeErrorCode::LedgerFailure,
    ] {
        let error = RuntimeClientError::rejected(
            "test_runtime_error",
            RuntimeErrorProjection::new(code, false),
        );
        assert!(!error.is_fallback_eligible());
        assert!(error.to_string().contains(&format!("{code:?}")));
    }
}

#[test]
fn post_terminal_projection_failure_preserves_committed_receipt() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request"),
        ids.mint_correlation_id().expect("correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        1,
        RuntimeOperation::Health,
    )
    .expect("request");
    let receipt = RuntimeReceipt::success(
        &request,
        RuntimeReceiptState::Completed,
        None,
        RuntimeResult::Health {
            owner_epoch: *ids.mint_owner_epoch().expect("epoch").transport(),
        },
    )
    .expect("receipt");
    let error = RuntimeClientError::after_commit(
        "runtime_projection_failed_after_terminal",
        "query_runtime_flow_projection",
        receipt.clone(),
        RuntimeClientError::fatal("runtime_connection_failed", "query_runtime_events"),
    );

    assert_eq!(error.committed_receipt(), Some(&receipt));
    assert!(error.is_fatal());
    assert!(error.to_string().contains("terminal receipt was committed"));
}
