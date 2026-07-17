// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::MONITOR_FILE_NAME;
use crate::time::unix_ms_now;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    AgentAttentionState, AgentPayload, AgentResponseDisposition, AgentSessionId,
    AgentSessionResponse, AgentWakeKind, ApplicationLifecycleAction, ApprovalDecisionRecord,
    ApprovalDisposition, ApprovalTarget, ArtifactKind, CaptureSequenceSpec,
    CatalogDeclarationPatch, CatalogPayload, CatalogProposal, ClientActionKind, ClientActionRecord,
    ClientActionValue, ContainedTaskRequest, EffectDisposition, EventActor, EventPayload,
    EventQuery, EventSeverity, EventSource, EventType, FactContent, FactRecord, FactScope,
    FactTtlPolicy, FactTtlSource, FactValue as ContractFactValue, IdentifierIssuer, InputAction,
    InstanceFactContext, InstanceId, IssuedCorrelationId, LeasePriority, LeaseQueuePolicy,
    LeaseQueueStatus, LeaseToken, MonitorDiagnosis, MonitorDisposition, MonitorObservation,
    MonitorPayload, MonitorRecoveryCoordinationReason, MonitorRecoveryKind, OriginModule,
    PerformanceControlLevel, PerformanceMonitorHealth, PolicyExecutionOutcome, PolicyFailureClass,
    PolicyFailureDisposition, PolicyPayload, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind, ProjectDecisionPageRequest, ProjectDecisionState,
    ProjectInterfaceRequest, ProjectedArtifactReference, ProjectionPayload, ProjectionProfile,
    ProposalClass, ProposalDisposition, ProposalDocument, ProposalKind, ProposalPatchOperation,
    PublicEventPayload, RUNTIME_INFO_FILE, ReleasePayload, ReleaseResourceVersion,
    ReleaseTransitionKind, ResourceAuthoringEvent, ResourceAuthoringPhase, RuntimeCaptureBackend,
    RuntimeErrorCode, RuntimeMonitorPolicy, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState,
    RuntimeReleaseSet, RuntimeRequest, RuntimeResult, StatePayload, StateRecoveryAction,
    StateValidationResult, TaskOutcome, TaskPayload, TaskSemanticFact, TaskTemplateInstantiation,
    TerminalEvent,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CohortBudgets, Comparison, DecisionReasonChain,
    DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime, FactValue,
    ForwardProjectionConfig, HostResourceSnapshot, InstanceSnapshot, LoadProfile,
    MaintenanceDisposition, MaintenanceTrendPolicy, MetricRef, ObservedFact, ObservedOutcome,
    OutlierMetric, OutlierPolicy, PoolValueSnapshot, PredicateSpec, ScopeSelector, StrategicBand,
    StrategicEvidencePointer, StrategicGoal, StrategicInstanceAssessment, StrategicReport,
    StrategicTemplate,
};
use actingcommand_runtime_state::{
    RUNTIME_STATE_DATABASE_FILE, RUNTIME_STATE_INTEGRITY_KEY_FILE, ReleaseArtifactSources,
    RuntimeStateStore,
};
use actingcommand_scheduler::{ConnectionId, SchedulerConfig};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const TEST_GOVERNANCE_CAPABILITY: &str = "runtime-host-governance-test-capability";

#[derive(Default)]
struct FakeState {
    open_count: AtomicUsize,
    input_count: AtomicUsize,
    close_count: AtomicUsize,
    fail_input: AtomicBool,
    block_input: AtomicBool,
    input_started: AtomicBool,
    capture_open_count: AtomicUsize,
    capture_count: AtomicUsize,
    capture_close_count: AtomicUsize,
    fail_capture: AtomicBool,
    fail_capture_on: AtomicUsize,
    transition_capture_after_input: AtomicBool,
    monitor_observation_count: AtomicUsize,
    monitor_mode: AtomicUsize,
    application_count: AtomicUsize,
    fail_application: AtomicBool,
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

struct FakeCapture {
    state: Arc<FakeState>,
    closed: bool,
}

impl FakeBackend {
    fn input(&self) -> DeviceResult<()> {
        self.state.input_started.store(true, Ordering::Release);
        while self.state.block_input.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        if self.state.fail_input.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected backend failure"));
        }
        self.state.input_count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.input()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.input()
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
        if !self.closed {
            self.closed = true;
            self.state.close_count.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let capture_number = self.state.capture_count.fetch_add(1, Ordering::AcqRel) + 1;
        if self.state.fail_capture.load(Ordering::Acquire)
            || self.state.fail_capture_on.load(Ordering::Acquire) == capture_number
        {
            return Err(DeviceError::fatal("injected capture failure"));
        }
        let first = if self
            .state
            .transition_capture_after_input
            .load(Ordering::Acquire)
            && self.state.input_count.load(Ordering::Acquire) > 0
        {
            [0, 0, 255]
        } else {
            [255, 0, 0]
        };
        Frame::from_pixels(
            2,
            1,
            [first.as_slice(), &[0, 255, 0]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            self.state
                .capture_close_count
                .fetch_add(1, Ordering::AcqRel);
        }
    }
}

struct FakeEntry {
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

struct FakeProvider {
    entries: BTreeMap<String, FakeEntry>,
    advertised_aliases: Option<Vec<String>>,
}

impl FakeProvider {
    fn one(alias: &str, instance_id: InstanceId, state: Arc<FakeState>) -> Self {
        Self::from_entries([(alias.to_string(), instance_id, state)])
    }

    fn from_entries(
        entries: impl IntoIterator<Item = (String, InstanceId, Arc<FakeState>)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(alias, instance_id, state)| (alias, FakeEntry { instance_id, state }))
                .collect(),
            advertised_aliases: None,
        }
    }

    fn with_inventory(mut self, aliases: impl IntoIterator<Item = String>) -> Self {
        self.advertised_aliases = Some(aliases.into_iter().collect());
        self
    }
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        self.advertised_aliases
            .clone()
            .unwrap_or_else(|| self.entries.keys().cloned().collect())
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        let entry = self.entries.get(instance_alias)?;
        Some(ResolvedExecutionInstance::new(
            entry.instance_id,
            "127.0.0.1:16384",
        ))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("fake instance is not registered"))?;
        entry.state.open_count.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&entry.state),
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        let entry = self
            .entries
            .get(instance_alias)
            .ok_or_else(|| DeviceError::fatal("fake instance is not registered"))?;
        entry
            .state
            .capture_open_count
            .fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&entry.state),
            closed: false,
        }))
    }

    fn observe_monitor(
        &self,
        instance_alias: &str,
        expected_page: &str,
        _frame: &Frame,
    ) -> actingcommand_execution_kernel::ExecutionKernelResult<MonitorObservation> {
        let entry = self
            .entries
            .get(instance_alias)
            .expect("resolved fake monitor instance");
        entry
            .state
            .monitor_observation_count
            .fetch_add(1, Ordering::AcqRel);
        let observation = match entry.state.monitor_mode.load(Ordering::Acquire) {
            0 => MonitorObservation::new(
                MonitorDiagnosis::Healthy,
                expected_page,
                Some(expected_page.to_string()),
            ),
            1 => MonitorObservation::new(MonitorDiagnosis::Standby, expected_page, None),
            2 => MonitorObservation::new(
                MonitorDiagnosis::UnexpectedPage,
                expected_page,
                Some("unexpected".to_string()),
            ),
            3 => MonitorObservation::new(
                MonitorDiagnosis::CaptureStaleSuspected,
                expected_page,
                None,
            ),
            _ => MonitorObservation::new(
                MonitorDiagnosis::Healthy,
                "wrong-policy-page",
                Some("wrong-policy-page".to_string()),
            ),
        }
        .expect("fake monitor observation must be valid");
        Ok(observation)
    }

    fn control_application(
        &self,
        instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .expect("resolved fake application instance");
        entry.state.application_count.fetch_add(1, Ordering::AcqRel);
        if entry.state.fail_application.load(Ordering::Acquire) {
            Err(DeviceError::fatal("private application failure"))
        } else {
            Ok(())
        }
    }
}

struct TestClient {
    stream: TcpStream,
    ids: IdentifierIssuer,
}

impl TestClient {
    fn connect(host: &RuntimeHost) -> Self {
        Self::connect_address(host.runtime_info().socket_addr().expect("runtime address"))
    }

    fn connect_state_root(root: &Path) -> Self {
        let bytes = fs::read(root.join(RUNTIME_INFO_FILE)).expect("runtime info bytes");
        let info: actingcommand_contract::RuntimeInfo =
            serde_json::from_slice(&bytes).expect("runtime info");
        Self::connect_address(info.socket_addr().expect("runtime address"))
    }

    fn connect_address(address: std::net::SocketAddr) -> Self {
        let stream = TcpStream::connect(address).expect("connect runtime");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("write timeout");
        stream.set_nodelay(true).expect("tcp nodelay");
        Self {
            stream,
            ids: IdentifierIssuer::new().expect("identifier issuer"),
        }
    }

    fn request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        let correlation_id = self.ids.mint_correlation_id().expect("correlation id");
        self.request_with_correlation(correlation_id, operation)
    }

    fn request_with_correlation(
        &self,
        correlation_id: IssuedCorrelationId,
        operation: RuntimeOperation,
    ) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            correlation_id,
            None,
            EventActor::Cli,
            EventSource::Cli,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("runtime request")
    }

    fn agent_request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            self.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::Agent,
            EventSource::Adapter,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("agent runtime request")
    }

    fn governance_request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            self.ids.mint_correlation_id().expect("correlation id"),
            None,
            EventActor::User,
            EventSource::Ui,
            unix_ms_now().expect("wall clock"),
            operation,
        )
        .expect("governance runtime request")
    }

    fn authenticate_governance(&mut self) {
        let request = self.governance_request(RuntimeOperation::AuthenticateGovernance {
            capability: TEST_GOVERNANCE_CAPABILITY.to_owned(),
        });
        let receipt = self.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::GovernanceAuthenticated)
        ));
    }

    fn send(&mut self, request: &RuntimeRequest) -> RuntimeReceipt {
        self.send_result(request).expect("runtime receipt")
    }

    fn send_result(&mut self, request: &RuntimeRequest) -> RuntimeHostResult<RuntimeReceipt> {
        write_frame(&mut self.stream, request, DEFAULT_RUNTIME_MAX_FRAME_BYTES)?;
        let FrameRead::Data(frame) = read_frame(&mut self.stream, DEFAULT_RUNTIME_MAX_FRAME_BYTES)?
        else {
            return Err(RuntimeHostError::request(
                "test_receipt_missing",
                "read_test_receipt",
                RuntimeErrorCode::ProtocolInvalid,
            ));
        };
        let receipt = serde_json::from_slice::<RuntimeReceipt>(&frame).map_err(|_| {
            RuntimeHostError::request(
                "test_receipt_invalid",
                "read_test_receipt",
                RuntimeErrorCode::ProtocolInvalid,
            )
        })?;
        receipt.validate().map_err(|_| {
            RuntimeHostError::request(
                "test_receipt_invalid",
                "read_test_receipt",
                RuntimeErrorCode::ProtocolInvalid,
            )
        })?;
        Ok(receipt)
    }

    fn acquire(&mut self, alias: &str) -> (RuntimeRequest, LeaseToken) {
        let request = self.request(RuntimeOperation::acquire_lease(
            alias,
            self.ids.mint_holder_id().expect("holder id"),
        ));
        let receipt = self.send(&request);
        let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
            panic!("expected lease grant");
        };
        (request, token.clone())
    }

    fn queue(
        &mut self,
        alias: &str,
        priority: LeasePriority,
        timeout_ms: u64,
    ) -> (RuntimeRequest, LeaseQueueStatus) {
        let request = self.request(RuntimeOperation::queue_lease(
            alias,
            self.ids.mint_holder_id().expect("holder id"),
            LeaseQueuePolicy::new(priority, timeout_ms).expect("queue policy"),
        ));
        let receipt = self.send(&request);
        let RuntimeResult::LeaseQueued { status } = receipt.result().expect("queue result") else {
            panic!("expected queued lease");
        };
        (request, status.clone())
    }
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn runtime_request(ids: &IdentifierIssuer, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        operation,
    )
    .expect("runtime request")
}

fn event_types_for_request(
    host: &RuntimeHost,
    ids: &IdentifierIssuer,
    connection_id: ConnectionId,
    request_id: actingcommand_contract::RequestId,
) -> Vec<EventType> {
    let query = runtime_request(
        ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
        },
    );
    let receipt = host
        .process_request_for_test(&query, connection_id)
        .expect("event query");
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    events.iter().map(|event| event.event_type).collect()
}

fn event_types_for_correlation(
    client: &mut TestClient,
    correlation_id: actingcommand_contract::CorrelationId,
) -> Vec<EventType> {
    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&query);
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    events.iter().map(|event| event.event_type).collect()
}

fn projected_events(
    client: &mut TestClient,
    query: EventQuery,
) -> Vec<actingcommand_contract::ProjectedEvent> {
    let request = client.request(RuntimeOperation::QueryEvents {
        query,
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&request);
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    events.clone()
}

fn projected_task_semantic_fact(
    event: &actingcommand_contract::ProjectedEvent,
) -> Option<&TaskSemanticFact> {
    match &event.payload {
        ProjectionPayload::Public(projected) => match projected.as_ref() {
            PublicEventPayload::Task(payload) => payload.task_semantic_fact(),
            _ => None,
        },
        ProjectionPayload::Full(projected) => match projected.as_ref() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
            _ => None,
        },
        _ => None,
    }
}

fn config(root: &TempDir) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root.path(), b"runtime-host-test-salt")
        .with_governance_capability(TEST_GOVERNANCE_CAPABILITY)
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

fn release_set(
    root: &Path,
    version: &str,
    marker: char,
) -> (RuntimeReleaseSet, ReleaseArtifactSources) {
    let source_root = root.join(format!("release-source-{version}-{marker}"));
    fs::create_dir(&source_root).expect("release source root");
    let runtime = source_root.join("runtime.bin");
    let ui = source_root.join("ui.bin");
    let resource = source_root.join("resource.bin");
    let runtime_bytes = format!("runtime:{version}:{marker}");
    let ui_bytes = format!("ui:{version}:{marker}");
    let resource_bytes = format!("resource:{version}:{marker}");
    fs::write(&runtime, runtime_bytes.as_bytes()).expect("runtime artifact");
    fs::write(&ui, ui_bytes.as_bytes()).expect("UI artifact");
    fs::write(&resource, resource_bytes.as_bytes()).expect("resource artifact");
    let manifest = RuntimeReleaseSet::new(
        version,
        format!("sha256:{:x}", Sha256::digest(runtime_bytes.as_bytes())),
        version,
        format!("sha256:{:x}", Sha256::digest(ui_bytes.as_bytes())),
        vec![
            ReleaseResourceVersion::new(
                "project-neutral",
                version,
                format!("sha256:{:x}", Sha256::digest(resource_bytes.as_bytes())),
            )
            .expect("resource version"),
        ],
    )
    .expect("release set");
    let sources = ReleaseArtifactSources::new(
        runtime,
        ui,
        BTreeMap::from([("project-neutral".to_owned(), resource)]),
    );
    (manifest, sources)
}

fn host_with_state(root: &TempDir, alias: &str, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(alias, instance_id(), state)),
    )
    .expect("runtime host")
}

const POLICY_INSTANCE_ALIAS: &str = "fixture-instance-a";
const POLICY_INSTANCE_ALIAS_B: &str = "fixture-instance-b";
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;

fn policy_sources(version: u64) -> CatalogSources {
    let mut sources = CatalogSources {
        tasks: CatalogDocumentSource::new(
            "memory://fixture/tasks.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json").to_vec(),
        ),
        pools: CatalogDocumentSource::new(
            "memory://fixture/pools.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json").to_vec(),
        ),
        activity: CatalogDocumentSource::new(
            "memory://fixture/activity.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                .to_vec(),
        ),
        timeline: CatalogDocumentSource::new(
            "memory://fixture/timeline.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                .to_vec(),
        ),
    };
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: serde_json::Value =
            serde_json::from_slice(&source.bytes).expect("policy fixture JSON");
        document["catalog"]["catalog_version"] = serde_json::json!(version);
        source.bytes = serde_json::to_vec_pretty(&document).expect("policy fixture bytes");
    }
    sources
}

fn budget_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("budget task fixture");
    tasks["tasks"][0]["expected_duration_ms"] = serde_json::json!(60000);
    tasks["tasks"][0]["cooldown_ms"] = serde_json::json!(0);
    tasks["tasks"][0]["loop_budget"] = serde_json::json!({
        "daily_limit": 4,
        "window_iteration_limit": 4,
        "max_runtime_ms": 300000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("budget task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("budget activity fixture");
    activity["profiles"][0]["daily_budget"] = serde_json::json!(10);
    activity["profiles"][0]["max_window_iterations"] = serde_json::json!(10);
    activity["profiles"][0]["session_max_ms"] = serde_json::json!(1000000);
    activity["profiles"][0]["minimum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["maximum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("budget activity bytes");
    sources
}

fn pending_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("pending task fixture");
    let mut second = tasks["tasks"][0].clone();
    second["id"] = serde_json::json!("fixture.observe-b");
    second["scope"] =
        serde_json::json!({"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS_B});
    second["procedure_ref"] = serde_json::json!("procedure.observe-b");
    second["feedback_stop"]["task_id"] = serde_json::json!("fixture.observe-b");
    second["produces"] = serde_json::json!([]);
    second["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("pending tasks array")
        .push(second);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("pending task bytes");
    sources
}

fn detection_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("detection task fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "ordinary.ready",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": 60000
    });
    let mut detection = tasks["tasks"][0].clone();
    detection["id"] = serde_json::json!("fixture.detect");
    detection["procedure_ref"] = serde_json::json!("procedure.detect");
    detection["priority"] = serde_json::json!(50);
    detection["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "detection.required",
        "comparison": "eq",
        "value": {"type": "boolean", "value": true},
        "max_age_ms": 60000
    });
    detection["feedback_stop"]["task_id"] = serde_json::json!("fixture.detect");
    detection["produces"] = serde_json::json!([]);
    detection["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("detection tasks array")
        .push(detection);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("detection task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("detection activity fixture");
    activity["profiles"][0]["detection_budget"] = serde_json::json!({
        "window_dispatch_limit": 2,
        "window_runtime_ms": 20000,
        "expected_duration_ms": 10000
    });
    sources.activity.bytes =
        serde_json::to_vec_pretty(&activity).expect("detection activity bytes");
    sources
}

fn strategy_policy_sources(version: u64) -> CatalogSources {
    let mut sources = policy_sources(version);
    let game_scope = serde_json::json!({"kind": "game", "game_id": "fixture-game-a"});
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("strategy task fixture");
    tasks["tasks"][0]["scope"] = game_scope.clone();
    tasks["tasks"][0]["trigger"]["predicates"][1]["scope"] = game_scope.clone();
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("strategy task bytes");
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("strategy pool fixture");
    pools["pools"][0]["scope"] = game_scope;
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("strategy pool bytes");
    sources
}

fn strategy_report(
    base: &CatalogGeneration,
    evidence: &ProjectedArtifactReference,
    as_of_ledger_position: u64,
) -> StrategicReport {
    let artifact_id = serde_json::to_value(evidence.artifact_id)
        .expect("artifact id JSON")
        .as_str()
        .expect("artifact id string")
        .to_owned();
    StrategicReport::new(
        "fixture-game-a",
        base.catalog_hash(),
        base.catalog_version(),
        base.catalog_version() + 1,
        as_of_ledger_position,
        POLICY_NOW_UNIX_MS,
        format!("sha256:{}", "d".repeat(64)),
        format!("sha256:{}", "e".repeat(64)),
        vec![StrategicEvidencePointer {
            artifact_id,
            sha256: evidence.sha256.clone(),
        }],
        vec![StrategicGoal {
            goal_id: "goal.primary".to_owned(),
            goal_version: 1,
            metric: MetricRef::Fact {
                fact_key: "resource.primary".to_owned(),
            },
            templates: vec![StrategicTemplate {
                template_id: "template.primary".to_owned(),
                task_template_ids: vec!["fixture.observe".to_owned()],
                activity_profile_template_id: "fixture-activity-game".to_owned(),
                eligibility: PredicateSpec::Fact {
                    scope: ScopeSelector::Game {
                        game_id: "fixture-game-a".to_owned(),
                    },
                    fact_key: "feature.enabled".to_owned(),
                    comparison: Comparison::Eq,
                    value: FactValue::Boolean(true),
                    max_age_ms: Some(60_000),
                },
                match_bands: vec![
                    StrategicBand::Actionable,
                    StrategicBand::InfeasibleBestEffort,
                ],
                minimum_urgency_milli: 0,
                maximum_urgency_milli: 1_000_000,
                strategic_weight_milli: 500,
                load_profile: LoadProfile::Weighted {
                    cpu_milli: 200,
                    gpu_milli: 100,
                    io_milli: 300,
                },
                risk_class: "standard".to_owned(),
                budget_class: "bounded".to_owned(),
            }],
            outlier_policy: OutlierPolicy {
                metric: OutlierMetric::Shortfall,
                mad_multiplier_milli: 2_000,
                top_n: 1,
            },
        }],
        vec![
            StrategicInstanceAssessment {
                goal_id: "goal.primary".to_owned(),
                instance_id: "fixture-instance-a".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                fact_snapshot_id: "snapshot:strategy-a".to_owned(),
                current_projection: Some(50),
                production_rate_per_hour: Some(100),
                target: 100,
                deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                available: true,
                capability_ids: vec!["operation.observe".to_owned()],
            },
            StrategicInstanceAssessment {
                goal_id: "goal.primary".to_owned(),
                instance_id: "fixture-instance-b".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                fact_snapshot_id: "snapshot:strategy-b".to_owned(),
                current_projection: Some(0),
                production_rate_per_hour: Some(10),
                target: 100,
                deadline_unix_ms: POLICY_NOW_UNIX_MS + 3_600_000,
                available: true,
                capability_ids: vec!["operation.observe".to_owned()],
            },
        ],
        CohortBudgets {
            max_active: 2,
            max_prompt: 1,
        },
    )
    .expect("strategic report")
}

fn evaluated_policy_dispatch(
    host: &RuntimeHost,
    trigger: PolicyTrigger,
) -> (PolicyCycle, DispatchIntent, DecisionReasonChain) {
    evaluated_policy_dispatch_at(host, trigger, POLICY_NOW_UNIX_MS, 7)
}

fn evaluated_policy_dispatch_at(
    host: &RuntimeHost,
    trigger: PolicyTrigger,
    unix_ms: u64,
    seed: u64,
) -> (PolicyCycle, DispatchIntent, DecisionReasonChain) {
    let cycle = host
        .evaluate_policy_cycle(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms,
                monotonic_ms: unix_ms,
            },
            seed,
            trigger,
        )
        .expect("evaluate policy dispatch");
    let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
    let intent = evaluation
        .dispatch_intents
        .first()
        .unwrap_or_else(|| panic!("dispatch intent: {evaluation:#?}"))
        .clone();
    let reason_chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    (cycle, intent, reason_chain)
}

fn policy_context(host: &RuntimeHost, intent: &DispatchIntent) -> PolicyAdmissionContext {
    PolicyAdmissionContext {
        fact_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: BTreeSet::from(["approval:fixture-a".to_owned()]),
        fencing_owner_epoch: host.runtime_info().owner_epoch(),
        now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
    }
}

fn record_policy_approval(host: &RuntimeHost, intent: &DispatchIntent) -> TerminalEvent {
    record_policy_approval_disposition(host, intent, ApprovalDisposition::Approved)
}

fn record_policy_approval_disposition(
    host: &RuntimeHost,
    intent: &DispatchIntent,
    disposition: ApprovalDisposition,
) -> TerminalEvent {
    let decision = ApprovalDecisionRecord::new(
        "approval:fixture-a",
        disposition,
        ApprovalTarget::Catalog {
            catalog_hash: intent.catalog_hash.clone(),
            catalog_version: intent.catalog_version,
        },
        "user_confirmed",
    )
    .expect("approval decision");
    let mut client = TestClient::connect(host);
    client.authenticate_governance();
    let request = client.governance_request(RuntimeOperation::RecordApprovalDecision { decision });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ApprovalDecisionRecorded {
            approval_id,
            disposition: recorded,
        }) if approval_id == "approval:fixture-a" && *recorded == disposition
    ));
    receipt.terminal().expect("approval terminal")
}

fn record_target_approval(client: &mut TestClient, approval_id: &str, target: ApprovalTarget) {
    client.authenticate_governance();
    let decision = ApprovalDecisionRecord::new(
        approval_id,
        ApprovalDisposition::Approved,
        target,
        "proposal_reviewed",
    )
    .expect("proposal approval");
    let request = client.governance_request(RuntimeOperation::RecordApprovalDecision { decision });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
}

fn proposal_version_patches(version: u64) -> Vec<CatalogDeclarationPatch> {
    [
        ProposalDocument::Tasks,
        ProposalDocument::Pools,
        ProposalDocument::Activity,
        ProposalDocument::Timeline,
    ]
    .into_iter()
    .map(|document| {
        CatalogDeclarationPatch::new(
            document,
            ProposalPatchOperation::Replace,
            "/catalog/catalog_version",
            Some(version.to_string()),
        )
        .expect("catalog version patch")
    })
    .collect()
}

fn unverified_report(
    reference: &ProjectedArtifactReference,
    ids: &IdentifierIssuer,
) -> ProjectedArtifactReference {
    let mut reference = reference.clone();
    reference.artifact_id = *ids
        .mint_artifact_id()
        .expect("unverified artifact id")
        .transport();
    let artifact_id = serde_json::to_value(reference.artifact_id)
        .expect("artifact id JSON")
        .as_str()
        .expect("artifact id string")
        .to_owned();
    reference.object_key = Some(format!(
        "artifacts/{}/{}.txt",
        &reference.sha256[7..9],
        artifact_id
    ));
    reference
}

fn verified_artifact_sequence(host: &RuntimeHost, reference: &ProjectedArtifactReference) -> u64 {
    let mut client = TestClient::connect(host);
    projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .find(|event| event.artifacts.iter().any(|artifact| artifact == reference))
    .expect("artifact verification event")
    .sequence
}

fn policy_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-a".to_owned(),
        facts: Vec::new(),
        outcomes: vec![ObservedOutcome {
            task_id: "fixture.observe".to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            outcome_key: "completed".to_owned(),
            value: FactValue::Boolean(false),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        }],
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        }],
    }
}

fn pending_policy_facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.fact_snapshot_id = "snapshot:pending-a".to_owned();
    facts.outcomes.push(ObservedOutcome {
        task_id: "fixture.observe-b".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        outcome_key: "completed".to_owned(),
        value: FactValue::Boolean(false),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
    });
    facts.instances.push(InstanceSnapshot {
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        server_id: "fixture-server-b".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-b".to_owned(),
        available: true,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    facts
}

fn detection_policy_facts(ordinary_ready: bool, snapshot_id: &str) -> EvaluationFacts {
    let mut facts = policy_facts();
    facts.fact_snapshot_id = snapshot_id.to_owned();
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        fact_key: "ordinary.ready".to_owned(),
        value: FactValue::Boolean(ordinary_ready),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        confidence_milli: 1_000,
    });
    facts
}

fn stored_fact(
    scope: FactScope,
    key: &str,
    value: ContractFactValue,
    source_snapshot_id: &str,
    invalidate_on: Vec<EventType>,
) -> FactRecord {
    FactRecord {
        scope,
        key: key.to_owned(),
        content: FactContent::Inline { value },
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: Some(POLICY_NOW_UNIX_MS + 60_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 1_000,
            maximum_ms: 120_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 900,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: source_snapshot_id.to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on,
    }
}

fn policy_resources() -> EvaluationResources {
    EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        }],
        hosts: vec![HostResourceSnapshot {
            host_id: "fixture-host-a".to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
        }],
    }
}

fn pending_policy_resources() -> EvaluationResources {
    let mut resources = policy_resources();
    resources.hosts.push(HostResourceSnapshot {
        host_id: "fixture-host-b".to_owned(),
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
    });
    resources
}

#[test]
fn forward_projection_reuses_policy_state_without_runtime_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.projection_only",
        ContractFactValue::Boolean(true),
        "snapshot:forward-read-only",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish projection fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate invalidating catalog");
    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    drop(client);

    let config = ForwardProjectionConfig::for_hours(2, 64).expect("projection config");
    let first = host
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            config,
        )
        .expect("forward projection");
    let second = host
        .project_policy_forward(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            17,
            config,
        )
        .expect("replayed forward projection");
    assert_eq!(first, second);
    assert!(!first.steps.is_empty());

    let mut client = TestClient::connect(&host);
    let after = projected_events(&mut client, EventQuery::default());
    assert_eq!(before, after);
    assert!(
        after
            .iter()
            .all(|event| event.event_type != EventType::FactInvalidated)
    );
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("synchronize fact snapshot");
    assert!(
        snapshot
            .records
            .iter()
            .all(|record| record.key != "resource.projection_only")
    );
    host.close().expect("close host");
}

#[test]
fn predictive_maintenance_reports_missing_evidence_without_a_signal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        POLICY_NOW_UNIX_MS,
        MaintenanceTrendPolicy::default(),
    )
    .expect("maintenance query");

    let assessment = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("maintenance assessment");
    assert_eq!(
        assessment.disposition,
        MaintenanceDisposition::EvidenceInsufficient
    );
    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::PolicyPlanningSignalObserved),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn predictive_maintenance_publishes_one_evidence_pinned_recheck_signal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let durations = [100_u64, 110, 200, 240];
    let confidences = [950_u16, 940, 800, 780];
    let fact_scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    let mut next_evaluation_at = POLICY_NOW_UNIX_MS;
    let mut last_observed_at = POLICY_NOW_UNIX_MS;

    for (index, (&duration_ms, &confidence_milli)) in
        durations.iter().zip(confidences.iter()).enumerate()
    {
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::clone(&state),
            )),
        )
        .expect("maintenance runtime host");
        if index == 0 {
            host.activate_policy_catalog(&policy_sources(1))
                .expect("activate policy catalog");
        }
        let mut facts = policy_facts();
        facts.fact_snapshot_id = format!("snapshot:maintenance-cycle-{index}");
        facts.outcomes[0].observed_at_unix_ms = next_evaluation_at;
        let cycle = host
            .evaluate_policy_cycle(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: next_evaluation_at,
                    monotonic_ms: next_evaluation_at,
                },
                100 + u64::try_from(index).expect("bounded maintenance index"),
                PolicyTrigger::Reconciliation,
            )
            .expect("maintenance policy evaluation");
        let evaluation = cycle.evaluation.expect("maintenance evaluation");
        let intent = evaluation
            .dispatch_intents
            .first()
            .expect("maintenance dispatch intent")
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("maintenance reason chain")
            .clone();
        if index == 0 {
            record_policy_approval(&host, &intent);
        }
        let admission = host
            .admit_policy_dispatch(
                &intent,
                &reasons,
                &PolicyAdmissionContext {
                    fact_ledger_position: intent.input_ledger_position,
                    fact_snapshot_id: intent.fact_snapshot_id.clone(),
                    approval_fact_ids: BTreeSet::new(),
                    fencing_owner_epoch: host.runtime_info().owner_epoch(),
                    now_unix_ms: next_evaluation_at,
                },
            )
            .expect("maintenance dispatch admission");
        let PolicyDispatchAdmission::Granted { admission, .. } = admission else {
            panic!("expected maintenance dispatch admission")
        };
        last_observed_at = next_evaluation_at + duration_ms;
        host.record_policy_dispatch_outcome(
            &intent.decision_id,
            last_observed_at,
            &PolicyExecutionInput::Succeeded,
        )
        .expect("maintenance execution outcome");
        host.publish_fact(FactRecord {
            scope: fact_scope.clone(),
            key: "resource.primary".to_owned(),
            content: FactContent::Inline {
                value: ContractFactValue::Integer(10),
            },
            observed_at_unix_ms: last_observed_at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli,
            source_detector: "detector.maintenance".to_owned(),
            source_snapshot_id: format!("snapshot:maintenance-{index}"),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "a".repeat(64),
            invalidate_on: Vec::new(),
        })
        .expect("maintenance confidence fact");
        next_evaluation_at = admission.activity.next_eligible_unix_ms + 1;
        host.close().expect("close maintenance runtime host");
    }

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::clone(&state),
        )),
    )
    .expect("assessment runtime host");
    let query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        fact_scope,
        "resource.primary",
        last_observed_at,
        MaintenanceTrendPolicy::default(),
    )
    .expect("maintenance query");
    let first = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("maintenance assessment");
    let second = host
        .assess_and_publish_predictive_maintenance(&query)
        .expect("replayed maintenance assessment");
    assert_eq!(first, second);
    assert_eq!(first.disposition, MaintenanceDisposition::RecheckSuggested);
    assert_eq!(first.duration_sample_count, 4);
    assert_eq!(first.confidence_sample_count, 4);
    let later_query = MaintenanceLedgerQuery::new(
        POLICY_INSTANCE_ALIAS,
        "fixture.observe",
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        last_observed_at + 1_000,
        MaintenanceTrendPolicy::default(),
    )
    .expect("later maintenance query");
    let later = host
        .assess_and_publish_predictive_maintenance(&later_query)
        .expect("later maintenance assessment");
    assert_eq!(later.assessment_id, first.assessment_id);

    let mut client = TestClient::connect(&host);
    let signals = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signals.len(), 1);
    assert_eq!(state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn project_interface_projects_runtime_domains_and_rejects_unknown_versions() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.current",
        ContractFactValue::Integer(5),
        "snapshot:project-interface",
        Vec::new(),
    ))
    .expect("publish fact");
    let (_, intent, reason_chain) = evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(
        &intent,
        &reason_chain,
        &PolicyAdmissionContext {
            fact_ledger_position: intent.input_ledger_position,
            fact_snapshot_id: intent.fact_snapshot_id.clone(),
            approval_fact_ids: BTreeSet::new(),
            fencing_owner_epoch: host.runtime_info().owner_epoch(),
            now_unix_ms: POLICY_NOW_UNIX_MS,
        },
    )
    .expect("admit dispatch");

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::current(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProjectInterface { response } = receipt.result().expect("result") else {
        panic!("expected project interface response");
    };
    let snapshot = response.snapshot();
    assert_eq!(
        snapshot.project.as_ref().expect("project").project_id,
        "fixture.catalog-a"
    );
    assert_eq!(snapshot.catalog.as_ref().expect("catalog").goal_count, 1);
    assert_eq!(snapshot.instances.len(), 1);
    assert_eq!(snapshot.facts.len(), 1);
    assert_eq!(snapshot.goals.len(), 1);
    assert_eq!(snapshot.decisions.len(), 1);
    assert_eq!(snapshot.decisions[0].state, ProjectDecisionState::Admitted);
    let decision_page = snapshot.decision_page.as_ref().expect("decision page");
    assert_eq!(decision_page.returned_count(), 1);
    assert!(!decision_page.has_more());
    assert_eq!(snapshot.approvals.len(), 1);
    assert!(!snapshot.runtime.fatal);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);

    let unsupported = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::new(vec![
            "actingcommand.project-interface.v9".to_owned(),
        ])
        .expect("well-formed version request"),
    });
    let rejected = client.send(&unsupported);
    assert_eq!(rejected.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        rejected.error_projection().expect("typed rejection").code,
        RuntimeErrorCode::ProtocolInvalid
    );
}

#[test]
fn project_interface_pages_decision_history_without_duplicates_or_loss() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");

    let (_, approval_basis, _) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::FactsChanged, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &approval_basis);
    let (_, admitted, admitted_reason) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        101,
    );
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted,
            &admitted_reason,
            &policy_context(&host, &admitted),
        )
        .expect("approved dispatch"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    record_policy_approval_disposition(&host, &admitted, ApprovalDisposition::Revoked);

    let mut expected = vec![admitted.decision_id.clone()];
    for index in 0..4_u64 {
        let (_, intent, reason_chain) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Reconciliation,
            POLICY_NOW_UNIX_MS + (index + 2) * 60_000,
            102 + index,
        );
        expected.push(intent.decision_id.clone());
        assert_eq!(
            host.admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
                .expect_err("unapproved dispatch must be rejected")
                .code(),
            "policy_approval_fact_missing"
        );
    }
    expected.reverse();

    let mut client = TestClient::connect(&host);
    let mut cursor = None;
    let mut collected = Vec::new();
    let mut collected_states = BTreeMap::new();
    for page_index in 0..4 {
        let request = ProjectInterfaceRequest::current()
            .with_decision_page(
                ProjectDecisionPageRequest::new(2, cursor.clone()).expect("page request"),
            )
            .expect("paged project request");
        let request = client.request(RuntimeOperation::ProjectInterface { request });
        let receipt = client.send(&request);
        let RuntimeResult::ProjectInterface { response } = receipt.result().expect("page result")
        else {
            panic!("expected project interface response")
        };
        let snapshot = response.snapshot();
        assert!(snapshot.decisions.len() <= 2);
        for decision in &snapshot.decisions {
            collected.push(decision.decision_id.clone());
            collected_states.insert(decision.decision_id.clone(), decision.state);
        }
        let page = snapshot.decision_page.as_ref().expect("decision page");
        assert_eq!(usize::from(page.returned_count()), snapshot.decisions.len());
        cursor = page.next_cursor().cloned();
        if page_index == 0 {
            host.complete_policy_dispatch(&admitted.decision_id)
                .expect("complete after snapshot");
        }
        if !page.has_more() {
            break;
        }
    }

    assert_eq!(collected, expected);
    assert_eq!(
        collected_states.get(&admitted.decision_id),
        Some(&ProjectDecisionState::Admitted),
        "later pages must remain bound to the first page ledger snapshot"
    );
    assert!(cursor.is_none());
    drop(client);
    host.close().expect("close host");
}

fn neutral_contained_task_package() -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{
                "schema_version":"Lab-1y.control.v1",
                "package_id":"neutral.semantic.task",
                "execution_mode":"navigable_route",
                "game":"neutral",
                "server":"test",
                "resolution":{"width":2,"height":1},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":1000,
                "max_steps":2
            }"#,
        ),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{
                "schema_version":"0.6",
                "task_id":"task",
                "game":"neutral",
                "server_scope":["test"],
                "coordinate_space":{"width":2,"height":1},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0}
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":2,"height":1},
                "defaults":{"color_max_distance":0.0},
                "targets":[
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                ]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (path, contents) in files {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_input_denied(client: &mut TestClient, token: LeaseToken, expected: RuntimeErrorCode) {
    let request = client.request(RuntimeOperation::Input {
        token,
        action: InputAction::Tap { x: 10, y: 20 },
    });
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(receipt.error_projection().expect("denial").code, expected);
}

fn concurrent_acquire(
    mut client: TestClient,
    alias: &'static str,
    start: Arc<Barrier>,
    completed: Arc<Barrier>,
) -> thread::JoinHandle<RuntimeReceipt> {
    thread::spawn(move || {
        let request = client.request(RuntimeOperation::acquire_lease(
            alias,
            client.ids.mint_holder_id().expect("holder id"),
        ));
        start.wait();
        let receipt = client.send(&request);
        completed.wait();
        receipt
    })
}

#[test]
fn runtime_status_lists_configured_instances_and_live_scheduler_state() {
    let root = TempDir::new().expect("tempdir");
    let state_a = Arc::new(FakeState::default());
    let state_b = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            ("node.c".to_string(), instance_id(), Arc::clone(&state_b)),
            ("node.a".to_string(), instance_id(), Arc::clone(&state_a)),
        ])),
    )
    .expect("runtime host");
    let mut owner = TestClient::connect(&host);

    let initial = owner
        .send_result(&owner.request(RuntimeOperation::Status))
        .expect("initial status receipt");
    let RuntimeResult::Status { status } = initial.result().expect("status result") else {
        panic!("expected runtime status");
    };
    assert_eq!(status.owner_epoch(), host.runtime_info().owner_epoch());
    assert_eq!(status.instances().len(), 2);
    assert_eq!(status.instances()[0].instance_alias(), "node.a");
    assert_eq!(status.instances()[1].instance_alias(), "node.c");
    assert!(
        status
            .instances()
            .iter()
            .all(|instance| !instance.lease_active())
    );
    assert_eq!(state_a.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state_b.open_count.load(Ordering::Acquire), 0);

    let acquire = owner.request(RuntimeOperation::acquire_lease(
        "node.a",
        owner.ids.mint_holder_id().expect("owner holder"),
    ));
    let acquire = owner.send(&acquire);
    assert!(matches!(
        acquire.result(),
        Some(RuntimeResult::LeaseGranted { .. })
    ));
    let mut waiter = TestClient::connect(&host);
    let queued = waiter.request(RuntimeOperation::queue_lease(
        "node.a",
        waiter.ids.mint_holder_id().expect("waiter holder"),
        LeaseQueuePolicy::new(LeasePriority::Normal, 1_000).expect("queue policy"),
    ));
    let queued = waiter.send(&queued);
    assert!(matches!(
        queued.result(),
        Some(RuntimeResult::LeaseQueued { .. })
    ));

    let live = owner
        .send_result(&owner.request(RuntimeOperation::Status))
        .expect("live status receipt");
    let RuntimeResult::Status { status } = live.result().expect("live status result") else {
        panic!("expected live runtime status");
    };
    let active = &status.instances()[0];
    assert!(active.lease_active());
    assert_eq!(active.queued_request_count(), 1);
    assert!(!active.takeover_cooldown_active());
    assert_eq!(state_a.open_count.load(Ordering::Acquire), 0);

    drop(waiter);
    drop(owner);
    host.close().expect("close host");
}

#[test]
fn runtime_registry_is_immutable_and_rejects_duplicate_instance_ids() {
    let root = TempDir::new().expect("tempdir");
    let hidden_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::from_entries([
                (
                    "node.a".to_string(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                ),
                (
                    "hidden.node".to_string(),
                    instance_id(),
                    Arc::clone(&hidden_state),
                ),
            ])
            .with_inventory(["node.a".to_string()]),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let hidden = client.request(RuntimeOperation::acquire_lease(
        "hidden.node",
        client.ids.mint_holder_id().expect("hidden holder"),
    ));
    let hidden = client.send(&hidden);
    assert_eq!(hidden.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        hidden.error_projection().expect("hidden denial").code,
        RuntimeErrorCode::InstanceUnknown
    );
    assert_eq!(hidden_state.open_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");

    let duplicate_root = TempDir::new().expect("duplicate tempdir");
    let duplicate_id = instance_id();
    let duplicate = RuntimeHost::start(
        config(&duplicate_root),
        Arc::new(FakeProvider::from_entries([
            (
                "node.a".to_string(),
                duplicate_id,
                Arc::new(FakeState::default()),
            ),
            (
                "node.c".to_string(),
                duplicate_id,
                Arc::new(FakeState::default()),
            ),
        ])),
    );
    let error = match duplicate {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("duplicate instance IDs must fail startup");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "duplicate_runtime_instance_id");
    assert!(error.is_fatal());
}

#[test]
fn runtime_monitor_policy_persists_and_idempotent_updates_do_not_rewrite_state() {
    let root = TempDir::new().expect("tempdir");
    let configured_id = instance_id();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            configured_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("policy");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: policy.clone(),
    });
    let configured = client.send(&configure);
    let RuntimeResult::MonitorConfigured { status } =
        configured.result().expect("configured result")
    else {
        panic!("expected configured monitor");
    };
    assert_eq!(status.policy(), Some(&policy));
    assert_eq!(
        event_types_for_request(
            &host,
            &client.ids,
            ConnectionId::new(99).expect("query connection"),
            configure.request_id()
        ),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
        ]
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0]
                    .state()
                    .is_some_and(|state| state.run_count() >= 1)
        )
    });
    let journal = root.path().join(MONITOR_FILE_NAME);
    let first_length = fs::metadata(&journal).expect("monitor metadata").len();

    let repeated = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: policy.clone(),
    });
    assert!(matches!(
        client.send(&repeated).result(),
        Some(RuntimeResult::MonitorConfigured { .. })
    ));
    assert_eq!(
        fs::metadata(&journal).expect("monitor metadata").len(),
        first_length
    );
    let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
    let RuntimeResult::MonitorStatus { status } = status.result().expect("monitor status") else {
        panic!("expected monitor status");
    };
    assert_eq!(status.instances().len(), 1);
    assert_eq!(status.instances()[0].policy(), Some(&policy));
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", configured_id, state)),
    )
    .expect("reopened runtime");
    let mut client = TestClient::connect(&reopened);
    let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
    let RuntimeResult::MonitorStatus { status } = status.result().expect("reopened status") else {
        panic!("expected reopened monitor status");
    };
    assert_eq!(status.instances()[0].policy(), Some(&policy));
    assert!(
        status.instances()[0]
            .state()
            .is_some_and(|state| state.run_count() >= 1)
    );

    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert!(matches!(
        client.send(&clear).result(),
        Some(RuntimeResult::MonitorCleared { status }) if status.policy().is_none()
    ));
    let cleared_length = fs::metadata(&journal).expect("monitor metadata").len();
    let repeated_clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert!(matches!(
        client.send(&repeated_clear).result(),
        Some(RuntimeResult::MonitorCleared { status }) if status.policy().is_none()
    ));
    assert_eq!(
        fs::metadata(&journal).expect("monitor metadata").len(),
        cleared_length
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn resident_monitor_runs_without_a_client_and_records_artifact_backed_lifecycle() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(200, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);

    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    let mut client = TestClient::connect(&host);
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0].state().is_some_and(|state| {
                    state.run_count() >= 1
                        && state.last_decision().is_some_and(|decision| {
                            decision.disposition() == MonitorDisposition::Healthy
                        })
                })
        )
    });

    let completed = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorProbeCompleted),
            ..EventQuery::default()
        },
    );
    let event = completed.last().expect("monitor completion event");
    let ProjectionPayload::Full(payload) = &event.payload else {
        panic!("expected full monitor completion payload");
    };
    let EventPayload::Monitor(MonitorPayload::Completed(detail)) = payload.as_ref() else {
        panic!("expected full monitor completion payload");
    };
    assert_eq!(detail.observation().diagnosis(), MonitorDiagnosis::Healthy);
    assert_eq!(detail.decision().disposition(), MonitorDisposition::Healthy);
    let run_id = *event.links.run_id().expect("monitor run id");
    let lifecycle = projected_events(
        &mut client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        lifecycle
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::MonitorProbeRequested,
            EventType::MonitorProbeStarted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
            EventType::ArtifactCreated,
            EventType::ArtifactVerified,
            EventType::CaptureCompleted,
            EventType::RecognitionCompleted,
            EventType::MonitorProbeCompleted,
        ]
    );
    let artifact = lifecycle
        .iter()
        .find(|event| event.event_type == EventType::ArtifactVerified)
        .and_then(|event| event.artifacts.first())
        .expect("verified monitor artifact");
    assert!(
        read_projected_verified(root.path(), artifact)
            .expect("monitor artifact bytes")
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    thread::sleep(Duration::from_millis(300));
    let observations = state.monitor_observation_count.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state.monitor_observation_count.load(Ordering::Acquire),
        observations
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn resident_monitor_uses_completion_based_cadence_without_a_tight_loop() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(100, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 3
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let started = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorProbeStarted),
            ..EventQuery::default()
        },
    );
    assert!(started.len() >= 3);
    for pair in started[..3].windows(2) {
        assert!(pair[1].timestamp_unix_ms - pair[0].timestamp_unix_ms >= 100);
    }
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_recovery_is_scheduler_admitted_without_executing_an_effect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.monitor_mode.store(1, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", true).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryAdmitted),
                ..EventQuery::default()
            },
        )
        .is_empty()
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let admitted = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorRecoveryAdmitted),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &admitted.last().expect("recovery admission").payload
    else {
        panic!("expected full recovery admission payload");
    };
    let EventPayload::Monitor(MonitorPayload::RecoveryAdmitted(detail)) = payload.as_ref() else {
        panic!("expected full recovery admission payload");
    };
    assert_eq!(detail.recovery(), MonitorRecoveryKind::WakeStandby);
    assert_eq!(
        detail.reason(),
        MonitorRecoveryCoordinationReason::SchedulerAvailable
    );
    assert_eq!(detail.effect_disposition(), EffectDisposition::NotPerformed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::TaskRequested),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_recovery_is_deferred_by_an_active_fenced_lease() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.monitor_mode.store(1, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", true).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryDeferred),
                ..EventQuery::default()
            },
        )
        .is_empty()
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);

    let deferred = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::MonitorRecoveryDeferred),
            ..EventQuery::default()
        },
    );
    let event = deferred.last().expect("recovery deferral");
    assert_eq!(event.links.lease_id(), Some(&token.lease_id()));
    let ProjectionPayload::Full(payload) = &event.payload else {
        panic!("expected full recovery deferral payload");
    };
    let EventPayload::Monitor(MonitorPayload::RecoveryDeferred(detail)) = payload.as_ref() else {
        panic!("expected full recovery deferral payload");
    };
    assert_eq!(detail.recovery(), MonitorRecoveryKind::WakeStandby);
    assert_eq!(
        detail.reason(),
        MonitorRecoveryCoordinationReason::ActiveLease
    );
    assert_eq!(detail.effect_disposition(), EffectDisposition::NotPerformed);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let release = client.request(RuntimeOperation::ReleaseLease {
        token: token.clone(),
    });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn monitor_capture_failure_is_persisted_without_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        let status = client.send(&client.request(RuntimeOperation::MonitorStatus));
        matches!(
            status.result(),
            Some(RuntimeResult::MonitorStatus { status })
                if status.instances()[0].state().is_some_and(|state| {
                    state.run_count() >= 1
                        && state.last_error() == Some(RuntimeErrorCode::CaptureFailed)
                        && state.last_decision().is_none()
                })
        )
    });
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    assert!(
        !projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorProbeFailed),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorProbeCompleted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryAdmitted),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::MonitorRecoveryDeferred),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_restart_fails_when_monitor_evidence_is_missing() {
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    wait_until(Duration::from_secs(2), || {
        state.monitor_observation_count.load(Ordering::Acquire) >= 1
    });
    let verified = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    );
    let object_key = verified
        .last()
        .and_then(|event| event.artifacts.first())
        .and_then(|artifact| artifact.object_key())
        .expect("monitor artifact object key")
        .to_string();
    let clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "node.a".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close host");
    fs::remove_file(root.path().join(object_key)).expect("remove monitor evidence");

    let restarted = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id, state)),
    );
    let error = match restarted {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("missing monitor evidence must fail restart");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "ledger_failure");
    assert!(error.is_fatal());
}

#[test]
fn invalid_monitor_provider_observation_poison_runtime_after_recording_failure() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.monitor_mode.store(usize::MAX, Ordering::Release);
    let host = host_with_state(&root, "node.a", state);
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "node.a".to_string(),
        policy: RuntimeMonitorPolicy::new(500, "home", false).expect("monitor policy"),
    });
    assert_eq!(
        client.send(&configure).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    wait_until(Duration::from_secs(2), || {
        host.fatal_error()
            .expect("runtime health")
            .is_some_and(|error| error.code() == "monitor_observation_invalid")
    });
    assert_eq!(
        host.close()
            .expect_err("invalid observation must fail host")
            .code(),
        "monitor_observation_invalid"
    );
}

#[test]
fn corrupt_monitor_registry_fails_runtime_startup() {
    let root = TempDir::new().expect("tempdir");
    fs::write(root.path().join(MONITOR_FILE_NAME), b"not-json\n")
        .expect("write monitor corruption");
    let result = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    );
    let error = match result {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("corrupt monitor registry must fail startup");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "monitor_record_invalid");
    assert!(error.is_fatal());
}

#[test]
fn zero_stagger_host_requests_produce_one_grant_and_one_busy_denial() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let completed = Arc::new(Barrier::new(3));
    let first = concurrent_acquire(first, "node.a", Arc::clone(&start), Arc::clone(&completed));
    let second = concurrent_acquire(second, "node.a", Arc::clone(&start), Arc::clone(&completed));

    start.wait();
    completed.wait();
    let receipts = [
        first.join().expect("first client"),
        second.join().expect("second client"),
    ];
    let grants = receipts
        .iter()
        .filter(|receipt| matches!(receipt.result(), Some(RuntimeResult::LeaseGranted { .. })))
        .count();
    let busy = receipts
        .iter()
        .filter(|receipt| {
            receipt
                .error_projection()
                .is_some_and(|error| error.code == RuntimeErrorCode::LeaseBusy)
        })
        .count();
    assert_eq!(grants, 1);
    assert_eq!(busy, 1);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    host.close().expect("close host");
}

#[test]
fn queued_release_transfers_only_after_the_durable_transfer_fact() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);
    assert!(!status.preempt_requested());

    let release = first.request(RuntimeOperation::ReleaseLease {
        token: old_token.clone(),
    });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let granted = second.send(&poll);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("transferred lease")
    else {
        panic!("expected transferred lease, got {:?}", granted.result());
    };
    assert_ne!(new_token.lease_id(), old_token.lease_id());
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    assert_eq!(
        event_types_for_correlation(&mut first, release.correlation_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn high_priority_queue_transfers_immediately_at_an_idle_safe_boundary() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let holder = second.ids.mint_holder_id().expect("holder id");
    let queued_request = second.request(RuntimeOperation::queue_lease(
        "node.a",
        holder,
        LeaseQueuePolicy::new(LeasePriority::High, 2_000).expect("queue policy"),
    ));

    let granted = second.send(&queued_request);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("idle preemption result")
    else {
        panic!("expected immediate idle transfer");
    };
    assert_eq!(granted.state(), RuntimeReceiptState::Admitted);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn high_priority_preemption_waits_for_the_durable_input_outcome() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.block_input.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("node.a");
    let input_token = old_token.clone();
    let input_thread = thread::spawn(move || {
        let input = first.request(RuntimeOperation::Input {
            token: input_token,
            action: InputAction::Reset,
        });
        let receipt = first.send(&input);
        (first, receipt)
    });
    wait_until(Duration::from_secs(2), || {
        state.input_started.load(Ordering::Acquire)
    });

    let (queued_request, status) = second.queue("node.a", LeasePriority::High, 2_000);
    assert!(status.preempt_requested());
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    assert!(matches!(
        second.send(&poll).result(),
        Some(RuntimeResult::LeasePending { .. })
    ));
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
        ]
    );

    state.block_input.store(false, Ordering::Release);
    let (mut first, input_receipt) = input_thread.join().expect("input thread");
    assert_eq!(input_receipt.state(), RuntimeReceiptState::Completed);
    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let granted = second.send(&poll);
    let RuntimeResult::LeaseGranted { token: new_token } =
        granted.result().expect("preempted lease")
    else {
        panic!("expected preempted lease");
    };
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_input_denied(&mut first, old_token, RuntimeErrorCode::LeaseMismatch);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerPreempted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease {
        token: new_token.clone(),
    });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(first);
    drop(second);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn queued_request_is_connection_bound_and_cancellation_is_ledger_visible() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let mut intruder = TestClient::connect(&host);
    let (_, token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);

    let poll = intruder.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let denied = intruder.send(&poll);
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        denied.error_projection().expect("poll denial").code,
        RuntimeErrorCode::QueueConnectionMismatch
    );
    let cancel = second.request(RuntimeOperation::CancelQueuedLease {
        queued_request_id: status.request_id(),
    });
    let cancelled = second.send(&cancel);
    assert_eq!(cancelled.state(), RuntimeReceiptState::Cancelled);
    assert!(matches!(
        cancelled.result(),
        Some(RuntimeResult::LeaseQueueCancelled { request_id, .. })
            if *request_id == status.request_id()
    ));
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    let release = first.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    drop(first);
    drop(second);
    drop(intruder);
    host.close().expect("close host");
}

#[test]
fn disconnect_promotes_another_connections_queue_without_opening_a_backend() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let _ = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 2_000);
    drop(first);

    let started = Instant::now();
    let new_token = loop {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "transfer timed out"
        );
        let poll = second.request(RuntimeOperation::PollQueuedLease {
            queued_request_id: status.request_id(),
        });
        let receipt = second.send(&poll);
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => break token.clone(),
            Some(RuntimeResult::LeasePending { .. }) => thread::sleep(Duration::from_millis(10)),
            other => panic!("unexpected disconnect transfer result: {other:?}"),
        }
    };
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease { token: new_token });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn lease_expiry_promotes_the_queue_and_fences_the_expired_token() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let provider = Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(
        config(&root).with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 200,
            ..SchedulerConfig::default()
        }),
        provider,
    )
    .expect("runtime host");
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, expired_token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 1_000);

    let started = Instant::now();
    let new_token = loop {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "expiry transfer timed out"
        );
        let poll = second.request(RuntimeOperation::PollQueuedLease {
            queued_request_id: status.request_id(),
        });
        let receipt = second.send(&poll);
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => break token.clone(),
            Some(RuntimeResult::LeasePending { .. }) => thread::sleep(Duration::from_millis(10)),
            other => panic!("unexpected expiry transfer result: {other:?}"),
        }
    };
    assert_input_denied(&mut first, expired_token, RuntimeErrorCode::LeaseMismatch);
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::LeaseTransitionIntent,
            EventType::LeaseTransferred,
        ]
    );
    let release = second.request(RuntimeOperation::ReleaseLease { token: new_token });
    assert_eq!(
        second.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(first);
    drop(second);
    host.close().expect("close host");
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
}

#[test]
fn queued_timeout_is_a_visible_terminal_denial() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, token) = first.acquire("node.a");
    let (queued_request, status) = second.queue("node.a", LeasePriority::Normal, 50);
    thread::sleep(Duration::from_millis(100));

    let poll = second.request(RuntimeOperation::PollQueuedLease {
        queued_request_id: status.request_id(),
    });
    let expired = second.send(&poll);
    assert_eq!(expired.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        expired.error_projection().expect("queue expiry").code,
        RuntimeErrorCode::QueueExpired
    );
    assert_eq!(
        event_types_for_correlation(&mut second, queued_request.correlation_id()),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerDenied,
        ]
    );
    let release = first.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(first.send(&release).state(), RuntimeReceiptState::Completed);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn different_instances_acquire_and_execute_independently() {
    let root = TempDir::new().expect("tempdir");
    let state_a = Arc::new(FakeState::default());
    let state_b = Arc::new(FakeState::default());
    let provider = FakeProvider::from_entries([
        ("node.a".to_string(), instance_id(), Arc::clone(&state_a)),
        ("node.c".to_string(), instance_id(), Arc::clone(&state_b)),
    ]);
    let host = RuntimeHost::start(config(&root), Arc::new(provider)).expect("runtime host");
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let run = |mut client: TestClient, alias: &'static str, start: Arc<Barrier>| {
        thread::spawn(move || {
            start.wait();
            let (_, token) = client.acquire(alias);
            let input = client.request(RuntimeOperation::Input {
                token: token.clone(),
                action: InputAction::Reset,
            });
            assert_eq!(client.send(&input).state(), RuntimeReceiptState::Completed);
            let release = client.request(RuntimeOperation::ReleaseLease { token });
            assert_eq!(
                client.send(&release).state(),
                RuntimeReceiptState::Completed
            );
        })
    };
    let first = run(first, "node.a", Arc::clone(&start));
    let second = run(second, "node.c", Arc::clone(&start));

    start.wait();
    first.join().expect("first instance client");
    second.join().expect("second instance client");
    for state in [&state_a, &state_b] {
        assert_eq!(state.open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    }
    host.close().expect("close host");
    for state in [&state_a, &state_b] {
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn readonly_observation_uses_one_correlation_and_typed_durable_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_string(),
        },
    );
    let completed = client.send(&observe);
    assert_eq!(completed.state(), RuntimeReceiptState::Completed);
    let observation = match completed.result() {
        Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => observation,
        other => panic!("unexpected observation result: {other:?}"),
    };
    assert_eq!((observation.width(), observation.height()), (2, 1));
    assert_eq!(
        observation.capture_backend(),
        RuntimeCaptureBackend::AdbScreencap
    );
    let artifact = read_projected_verified(root.path(), observation.artifact())
        .expect("verified observation artifact");
    assert!(artifact.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
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
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
}

#[test]
fn enabled_performance_monitor_collects_runtime_capture_pipeline_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root).with_performance_monitor(PerformanceMonitorConfig::default()),
        Arc::new(FakeProvider::one("ak.cn", instance_id(), state)),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "ak.cn".to_owned(),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    let observed_at_unix_ms = unix_ms_now().expect("wall clock");
    let context = host
        .performance_context_for_test("ak.cn", observed_at_unix_ms)
        .expect("performance context");
    assert!(context.max_capture_latency_ms.is_some());
    assert!(context.max_recognition_latency_ms.is_some());
    assert_eq!(context.max_action_effect_latency_ms, None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn bounded_capture_sequence_returns_unique_verified_observations_without_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let capture = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "node.a".to_string(),
            spec: CaptureSequenceSpec::new(3, 5).expect("sequence spec"),
        },
    );

    let completed = client.send(&capture);

    assert_eq!(completed.state(), RuntimeReceiptState::Completed);
    let sequence = match completed.result() {
        Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence,
        other => panic!("unexpected sequence result: {other:?}"),
    };
    assert_eq!(sequence.observations().len(), 3);
    let artifact_ids = sequence
        .observations()
        .iter()
        .map(|observation| observation.artifact().artifact_id)
        .collect::<BTreeSet<_>>();
    let frame_ids = sequence
        .observations()
        .iter()
        .map(|observation| *observation.artifact().frame_id().expect("frame id"))
        .collect::<BTreeSet<_>>();
    let object_keys = sequence
        .observations()
        .iter()
        .map(|observation| {
            observation
                .artifact()
                .object_key()
                .expect("object key")
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(artifact_ids.len(), 3);
    assert_eq!(frame_ids.len(), 3);
    assert_eq!(object_keys.len(), 3);
    for observation in sequence.observations() {
        assert!(
            read_projected_verified(root.path(), observation.artifact())
                .expect("verified sequence artifact")
                .starts_with(b"\x89PNG\r\n\x1a\n")
        );
    }
    let events = event_types_for_correlation(&mut client, correlation_id);
    assert_eq!(
        events,
        [
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::SchedulerAdmitted,
        ]
        .into_iter()
        .chain(
            [
                EventType::CaptureRequested,
                EventType::RecognitionRequested,
                EventType::ArtifactCreated,
                EventType::ArtifactVerified,
                EventType::CaptureCompleted,
                EventType::RecognitionCompleted,
            ]
            .into_iter()
            .cycle()
            .take(18),
        )
        .collect::<Vec<_>>()
    );
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 3);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
}

#[test]
fn capture_sequence_partial_failure_keeps_evidence_without_fake_success_or_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture_on.store(2, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "node.a".to_string(),
            spec: CaptureSequenceSpec::new(3, 0).expect("sequence spec"),
        },
    );

    let failed = client.send(&request);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(failed.result().is_none());
    assert_eq!(
        failed.error_projection().expect("capture failure").code,
        RuntimeErrorCode::CaptureFailed
    );
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ArtifactVerified)
            .count(),
        1
    );
    assert_eq!(
        events.last().map(|event| event.event_type),
        Some(EventType::RecognitionFailed)
    );
    assert!(!events.iter().any(|event| matches!(
        event.event_type,
        EventType::InputIntent | EventType::InputCommitted | EventType::InputFailed
    )));
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn capture_sequence_bounds_are_rejected_before_backend_open() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let valid = client.request(RuntimeOperation::CaptureSequence {
        instance_alias: "node.a".to_string(),
        spec: CaptureSequenceSpec::new(1, 0).expect("valid spec"),
    });
    let mut encoded = serde_json::to_value(valid).expect("request JSON");
    encoded["operation"]["spec"]["frame_count"] = serde_json::json!(61);
    let invalid = serde_json::from_value::<RuntimeRequest>(encoded).expect("wire request");

    let denied = client.send(&invalid);

    assert_eq!(denied.state(), RuntimeReceiptState::Denied);
    assert!(denied.result().is_none());
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 0);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn readonly_artifact_store_failure_is_fatal_without_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    fs::write(root.path().join("artifacts"), b"blocks artifact directory")
        .expect("block artifact directory");
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "node.a".to_string(),
    });

    let failed = client.send(&observe);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert!(failed.result().is_none());
    let error = failed.error_projection().expect("fatal artifact error");
    assert!(error.fatal);
    assert_eq!(error.code, RuntimeErrorCode::RuntimeFatal);
    assert_eq!(
        host.fatal_error()
            .expect("runtime fatal state")
            .expect("fatal error")
            .code(),
        "artifact_store_failure"
    );
    drop(client);
    assert_eq!(
        host.close()
            .expect_err("fatal host closes with failure")
            .code(),
        "artifact_store_failure"
    );
}

#[test]
fn readonly_failures_are_visible_and_terminal_without_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "node.a".to_string(),
        },
    );
    let failed = client.send(&observe);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        failed.error_projection().expect("failure").code,
        RuntimeErrorCode::CaptureFailed
    );
    assert!(failed.result().is_none());
    let events = event_types_for_correlation(&mut client, correlation_id);
    assert_eq!(
        &events[events.len() - 2..],
        [EventType::CaptureFailed, EventType::RecognitionFailed]
    );
    assert_eq!(state.capture_open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_close_count.load(Ordering::Acquire), 1);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}

#[test]
fn safe_reset_owns_lease_input_and_release_under_one_correlation() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::safe_reset("node.a", client.ids.mint_holder_id().expect("holder")),
    );
    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::SafeResetCompleted { .. })
    ));
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn application_lifecycle_owns_lease_effect_and_release_under_one_correlation() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::application_lifecycle(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ApplicationLifecycleAction::Restart,
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ApplicationLifecycleCompleted {
            action: ApplicationLifecycleAction::Restart,
            ..
        })
    ));
    assert_eq!(state.application_count.load(Ordering::Acquire), 1);
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::SchedulerAdmitted,
            EventType::ApplicationIntent,
            EventType::ApplicationCompleted,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn application_lifecycle_is_denied_while_another_client_holds_the_instance() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let mut owner = TestClient::connect(&host);
    let mut contender = TestClient::connect(&host);
    let (_request, token) = owner.acquire("neutral.instance");
    let request = contender.request(RuntimeOperation::application_lifecycle(
        "neutral.instance",
        contender.ids.mint_holder_id().expect("holder"),
        ApplicationLifecycleAction::Stop,
    ));

    let receipt = contender.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(state.application_count.load(Ordering::Acquire), 0);

    let release = owner.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(owner.send(&release).state(), RuntimeReceiptState::Completed);
    drop(owner);
    drop(contender);
    host.close().expect("close host");
}

#[test]
fn runtime_executes_neutral_contained_task_without_lab_ownership() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected.clone())
                .expect("task request"),
        ),
    );

    let receipt = client.send(&request);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            outcome: TaskOutcome::Success,
            final_page: Some(page),
            executed_steps: 1,
            ..
        }) if page == "neutral/terminal"
    ));
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.capture_count.load(Ordering::Acquire), 2);
    let event_types = event_types_for_correlation(&mut client, correlation_id);
    for required in [
        EventType::TaskRequested,
        EventType::TaskStarted,
        EventType::CaptureCompleted,
        EventType::TaskEvidenceIndexed,
        EventType::TaskRecognitionStarted,
        EventType::RecognitionCompleted,
        EventType::TaskRecognitionCompleted,
        EventType::TaskStepStarted,
        EventType::TaskEffectIntent,
        EventType::InputIntent,
        EventType::InputCommitted,
        EventType::TaskEffectCompleted,
        EventType::TaskStepFinished,
        EventType::TaskTerminalIntent,
        EventType::TaskCompleted,
    ] {
        assert!(event_types.contains(&required), "missing {required:?}");
    }
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let semantic = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .collect::<Vec<_>>();
    assert!(semantic.iter().any(|fact| matches!(
        fact,
        TaskSemanticFact::PackageAdmitted { package_sha256, .. }
            if package_sha256 == &expected
    )));
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::EvidenceIndexed { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::RecognitionStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::RecognitionCompleted { .. }))
            .count(),
        2
    );
    assert_eq!(
        semantic
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalCommitted { .. }))
            .count(),
        1
    );
    let evidence_frames = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEvidenceIndexed)
        .map(|event| *event.links.frame_id().expect("evidence frame id"))
        .collect::<BTreeSet<_>>();
    let verified_frames = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .map(|event| *event.links.frame_id().expect("artifact frame id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(evidence_frames, verified_frames);
    assert_eq!(evidence_frames.len(), 2);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn contained_task_replay_without_connection_cache_reuses_runtime_terminal() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package();
    fs::write(&package, &bytes).expect("write package");
    let expected = actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string();
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string(), expected)
                .expect("task request"),
        ),
    );
    let connection = ConnectionId::new(91).expect("connection");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first contained task");
    let replayed = host
        .process_request_for_test(&request, connection)
        .expect("replayed contained task");

    assert_eq!(replayed, first);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    let mut query_client = TestClient::connect(&host);
    let events = projected_events(
        &mut query_client,
        EventQuery {
            request_id: Some(request.request_id()),
            ..EventQuery::default()
        },
    );
    let facts = events.iter().filter_map(projected_task_semantic_fact);
    let mut packages = 0;
    let mut effect_intents = 0;
    let mut terminals = 0;
    for fact in facts {
        match fact {
            TaskSemanticFact::PackageAdmitted { .. } => packages += 1,
            TaskSemanticFact::EffectIntent { .. } => effect_intents += 1,
            TaskSemanticFact::TerminalCommitted { .. } => terminals += 1,
            _ => {}
        }
    }
    assert_eq!((packages, effect_intents, terminals), (1, 1, 1));
    drop(query_client);
    host.close().expect("close host");
}

#[test]
fn contained_task_terminal_is_absorbing_and_rejections_are_audited() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "neutral.instance", state);
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("neutral.instance");
    let request = client.request(RuntimeOperation::Health);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let task_id = ids.mint_task_id().expect("task id");
    let run_id = ids.mint_run_id().expect("run id");

    let committed = host
        .append_contained_task_terminal_for_test(
            &request,
            &token,
            task_id,
            run_id,
            TaskOutcome::Success,
            false,
            Some("neutral/terminal".to_string()),
            1,
            None,
        )
        .expect("first terminal");
    let (rejected_state, rejected_code, rejected_terminal) = host
        .append_contained_task_terminal_for_test(
            &request,
            &token,
            task_id,
            run_id,
            TaskOutcome::Failure,
            true,
            None,
            1,
            Some("conflicting_terminal"),
        )
        .expect_err("second terminal must be rejected");

    assert_eq!(rejected_state, RuntimeReceiptState::Denied);
    assert_eq!(rejected_code, RuntimeErrorCode::InvalidRequest);
    assert_ne!(committed, rejected_terminal.expect("rejection terminal"));
    let events = projected_events(
        &mut client,
        EventQuery {
            task_id: Some(*task_id.transport()),
            run_id: Some(*run_id.transport()),
            ..EventQuery::default()
        },
    );
    let facts = events
        .iter()
        .filter_map(projected_task_semantic_fact)
        .collect::<Vec<_>>();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalCommitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| matches!(fact, TaskSemanticFact::TerminalRejected { .. }))
            .count(),
        1
    );
    let release = client.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn safe_reset_replay_without_connection_cache_does_not_repeat_input() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::safe_reset("node.a", ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(77).expect("connection");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first safe reset");
    let replayed = host
        .process_request_for_test(&request, connection)
        .expect("replayed safe reset");

    assert_eq!(replayed, first);
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    assert_eq!(
        event_types_for_request(&host, &ids, connection, request.request_id()).len(),
        13
    );
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn safe_reset_replay_recovers_from_durable_ledger_after_host_restart() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let fixed_instance = instance_id();
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::safe_reset("node.a", ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(88).expect("connection");
    let first = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            fixed_instance,
            Arc::clone(&state),
        )),
    )
    .expect("first Runtime host");
    let first_receipt = first
        .process_request_for_test(&request, connection)
        .expect("first safe reset");
    first.close().expect("close first host");

    let second = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            fixed_instance,
            Arc::clone(&state),
        )),
    )
    .expect("second Runtime host");
    let replayed = second
        .process_request_for_test(&request, connection)
        .expect("durable replay");

    assert_eq!(replayed, first_receipt);
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    second.close().expect("close second host");
}

#[test]
fn typed_ipc_routes_input_once_and_correlates_ledger_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let health = client.request(RuntimeOperation::Health);
    let health = client.send_result(&health);
    assert!(
        health.is_ok(),
        "health failed: {health:?}; fatal={:?}",
        host.fatal_error()
    );
    let acquire_request = client.request(RuntimeOperation::acquire_lease(
        "node.a",
        client.ids.mint_holder_id().expect("holder id"),
    ));
    let acquire_receipt = client.send_result(&acquire_request);
    assert!(
        acquire_receipt.is_ok(),
        "acquire failed: {acquire_receipt:?}; fatal={:?}",
        host.fatal_error()
    );
    let acquire_receipt = acquire_receipt.expect("acquire receipt");
    assert_eq!(client.send(&acquire_request), acquire_receipt);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    let RuntimeResult::LeaseGranted { token } = acquire_receipt.result().expect("lease result")
    else {
        panic!("expected lease grant");
    };
    let token = token.clone();
    let renew_request = client.request(RuntimeOperation::RenewLease {
        token: token.clone(),
    });
    let renew_receipt = client.send(&renew_request);
    assert_eq!(client.send(&renew_request), renew_receipt);
    let RuntimeResult::LeaseRenewed { token } = renew_receipt.result().expect("renew result")
    else {
        panic!("expected renewed lease");
    };
    let token = token.clone();

    let actions = vec![
        InputAction::Tap { x: 10, y: 20 },
        InputAction::LongTap {
            x: 30,
            y: 40,
            duration_ms: 100,
        },
        InputAction::Swipe {
            x1: 10,
            y1: 20,
            x2: 30,
            y2: 40,
            duration_ms: 100,
        },
        InputAction::Key {
            key: "BACK".to_string(),
        },
        InputAction::Text {
            text: "highly-secret-input".to_string(),
        },
        InputAction::Reset,
    ];
    let mut text_request = None;
    for action in actions {
        let request = client.request(RuntimeOperation::Input {
            token: token.clone(),
            action: action.clone(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        if matches!(action, InputAction::Text { .. }) {
            text_request = Some((request, receipt));
        }
    }
    let (text_request, text_receipt) = text_request.expect("text request");
    assert_eq!(client.send(&text_request), text_receipt);
    assert_eq!(state.input_count.load(Ordering::Acquire), 6);

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(acquire_request.correlation_id()),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&query);
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    let event_types = events
        .iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
        ]
    );

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(text_request.correlation_id()),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&query);
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected input event projection");
    };
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
        ]
    );

    let all_events = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery::default(),
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&all_events);
    let encoded = serde_json::to_string(receipt.result().expect("events")).expect("encode events");
    assert!(!encoded.contains("highly-secret-input"));
    assert!(!encoded.contains("127.0.0.1:16384"));

    let release = client.request(RuntimeOperation::ReleaseLease {
        token: token.clone(),
    });
    let receipt = client.send(&release);
    assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    assert_eq!(client.send(&release), receipt);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
}

#[test]
fn one_correlation_queries_the_complete_lease_input_release_sequence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let mut client = TestClient::connect(&host);
    let correlation_id = client.ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation_id.transport();
    let acquire = client.request_with_correlation(
        correlation_id,
        RuntimeOperation::acquire_lease("node.a", client.ids.mint_holder_id().expect("holder id")),
    );
    let acquire_id = acquire.request_id();
    let receipt = client.send(&acquire);
    let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
        panic!("expected lease grant");
    };
    let token = token.clone();

    let input = client.request_with_correlation(
        correlation_id,
        RuntimeOperation::Input {
            token: token.clone(),
            action: InputAction::Tap { x: 10, y: 20 },
        },
    );
    let input_id = input.request_id();
    assert_eq!(client.send(&input).state(), RuntimeReceiptState::Completed);

    let release =
        client.request_with_correlation(correlation_id, RuntimeOperation::ReleaseLease { token });
    let release_id = release.request_id();
    assert_eq!(
        client.send(&release).state(),
        RuntimeReceiptState::Completed
    );

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            correlation_id: Some(correlation_transport),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Forensic,
    });
    let receipt = client.send(&query);
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert!(
        events[..4]
            .iter()
            .all(|event| event.links.request_id() == Some(&acquire_id))
    );
    assert!(
        events[4..7]
            .iter()
            .all(|event| event.links.request_id() == Some(&input_id))
    );
    assert!(
        events[7..]
            .iter()
            .all(|event| event.links.request_id() == Some(&release_id))
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_is_the_single_writer_for_one_correlated_resource_authoring_sequence() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation.transport();
    let connection = ConnectionId::new(121).expect("connection");
    let phases = [
        (ResourceAuthoringPhase::AuthoringStarted, None),
        (ResourceAuthoringPhase::DraftBuilt, None),
        (ResourceAuthoringPhase::ValidationCompleted, None),
        (ResourceAuthoringPhase::PromoteIntent, None),
        (ResourceAuthoringPhase::Promoted, None),
    ];

    for (phase, failure_code) in phases {
        let request = RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            correlation,
            None,
            EventActor::Lab,
            EventSource::Lab,
            unix_ms_now().expect("wall clock"),
            RuntimeOperation::RecordAuthoringEvent {
                event: ResourceAuthoringEvent::new(
                    phase,
                    "draft-a",
                    "resource-root",
                    "b".repeat(64),
                    vec!["operations/task-a/task.json".to_string()],
                    failure_code,
                )
                .expect("authoring event"),
            },
        )
        .expect("authoring request");
        let receipt = host
            .process_request_for_test(&request, connection)
            .expect("authoring receipt");
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(receipt.terminal().is_some());
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::AuthoringEventRecorded { phase: recorded }) if *recorded == phase
        ));
    }

    let query = runtime_request(
        &ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                correlation_id: Some(correlation_transport),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
        },
    );
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("event query");
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected events");
    };
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
    for event in events {
        assert_eq!(event.origin.source(), EventSource::Lab);
        assert_eq!(event.origin.module(), OriginModule::ResourceTooling);
        assert_eq!(event.origin.actor(), EventActor::Lab);
        assert!(matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(payload.as_ref(), EventPayload::ResourceAuthoring(_))
        ));
    }
    host.close().expect("close host");
}

#[test]
fn runtime_rejects_forged_non_lab_resource_authoring_ingress_without_ledger_effect() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", state);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation = ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation.transport();
    let valid = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        correlation,
        None,
        EventActor::Lab,
        EventSource::Lab,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordAuthoringEvent {
            event: ResourceAuthoringEvent::new(
                ResourceAuthoringPhase::AuthoringStarted,
                "draft-a",
                "resource-root",
                "b".repeat(64),
                vec!["operations/task-a/task.json".to_string()],
                None,
            )
            .expect("authoring event"),
        },
    )
    .expect("Lab request");
    let mut forged = serde_json::to_value(valid).expect("request JSON");
    forged["actor"] = serde_json::json!("cli");
    forged["source"] = serde_json::json!("cli");
    let forged: RuntimeRequest = serde_json::from_value(forged).expect("wire request");
    let connection = ConnectionId::new(122).expect("connection");
    let denied = host
        .process_request_for_test(&forged, connection)
        .expect("denied receipt");
    assert_eq!(denied.state(), RuntimeReceiptState::Denied);

    let query = runtime_request(
        &ids,
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                correlation_id: Some(correlation_transport),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
        },
    );
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("event query");
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected events");
    };
    assert!(events.is_empty());
    host.close().expect("close host");
}

#[test]
fn typed_client_action_is_idempotent_and_public_projection_hides_the_value() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "ak.cn", Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let secret_hash = format!("sha256:{}", "e".repeat(64));
    let request = client.request(RuntimeOperation::RecordClientAction {
        action: ClientActionRecord::new(
            "settings",
            "account_token",
            ClientActionKind::Input,
            Some("ak.cn".to_owned()),
            Some(ClientActionValue::Redacted {
                sha256: secret_hash.clone(),
                byte_count: 24,
            }),
        )
        .expect("client action"),
    });
    let first = client.send(&request);
    let replay = client.send(&request);
    assert_eq!(first, replay);
    assert!(matches!(
        first.result(),
        Some(RuntimeResult::ClientActionRecorded)
    ));

    let query = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery {
            event_type: Some(EventType::ClientAction),
            ..EventQuery::default()
        },
        profile: ProjectionProfile::Ui,
    });
    let receipt = client.send(&query);
    let RuntimeResult::Events { events } = receipt.result().expect("events") else {
        panic!("expected events")
    };
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Public(payload) = &events[0].payload else {
        panic!("expected public projection")
    };
    let PublicEventPayload::Client(payload) = payload.as_ref() else {
        panic!("expected client projection")
    };
    assert_eq!(payload.client_surface_id(), Some("settings"));
    assert_eq!(payload.client_control_id(), Some("account_token"));
    assert!(
        !serde_json::to_string(&events)
            .expect("events JSON")
            .contains(&secret_hash)
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn client_fact_request_id_cannot_cross_typed_operation_boundaries() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "fixture-instance-a", Arc::new(FakeState::default()));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request_id = ids.mint_request_id().expect("request id");
    let action = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("action correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordClientAction {
            action: ClientActionRecord::new(
                "settings",
                "refresh",
                ClientActionKind::Button,
                None,
                None,
            )
            .expect("client action"),
        },
    )
    .expect("action request");
    let action_other_correlation = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("other correlation"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        action.operation().clone(),
    )
    .expect("action request with another correlation");
    let approval = RuntimeRequest::new(
        request_id,
        ids.mint_correlation_id().expect("approval correlation"),
        None,
        EventActor::User,
        EventSource::Ui,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::RecordApprovalDecision {
            decision: ApprovalDecisionRecord::new(
                "approval:request-boundary",
                ApprovalDisposition::Approved,
                ApprovalTarget::Catalog {
                    catalog_hash: format!("sha256:{}", "a".repeat(64)),
                    catalog_version: 1,
                },
                "user_confirmed",
            )
            .expect("approval decision"),
        },
    )
    .expect("approval request");
    let connection = ConnectionId::new(99).expect("connection id");
    let authentication = RuntimeRequest::new(
        ids.mint_request_id().expect("authentication request"),
        ids.mint_correlation_id()
            .expect("authentication correlation"),
        None,
        EventActor::User,
        EventSource::Ui,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::AuthenticateGovernance {
            capability: TEST_GOVERNANCE_CAPABILITY.to_owned(),
        },
    )
    .expect("authentication request");
    assert_eq!(
        host.process_request_for_test(&authentication, connection)
            .expect("authentication receipt")
            .state(),
        RuntimeReceiptState::Completed
    );

    assert_eq!(
        host.process_request_for_test(&action, connection)
            .expect("action receipt")
            .state(),
        RuntimeReceiptState::Completed
    );
    let correlation_collision = host
        .process_request_for_test(&action_other_correlation, connection)
        .expect("correlation collision receipt");
    assert_eq!(correlation_collision.state(), RuntimeReceiptState::Denied);
    let collision = host
        .process_request_for_test(&approval, connection)
        .expect("collision receipt");
    assert_eq!(collision.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        collision.error_projection().expect("collision error").code,
        RuntimeErrorCode::InvalidRequest
    );
    assert_eq!(
        event_types_for_request(&host, &ids, connection, action.request_id()),
        vec![EventType::ClientAction]
    );
    host.close().expect("close host");
}

#[test]
fn concurrent_approval_targets_commit_exactly_one_authoritative_fact() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, "fixture-instance-a", Arc::new(FakeState::default()));
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let run = |mut client: TestClient, marker: char, start: Arc<Barrier>| {
        thread::spawn(move || {
            client.authenticate_governance();
            let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
                decision: ApprovalDecisionRecord::new(
                    "approval:concurrent-target",
                    ApprovalDisposition::Approved,
                    ApprovalTarget::Catalog {
                        catalog_hash: format!("sha256:{}", marker.to_string().repeat(64)),
                        catalog_version: 1,
                    },
                    "user_confirmed",
                )
                .expect("approval decision"),
            });
            start.wait();
            client.send(&request)
        })
    };
    let first = run(first, 'a', Arc::clone(&start));
    let second = run(second, 'b', Arc::clone(&start));
    start.wait();
    let receipts = [
        first.join().expect("first approval writer"),
        second.join().expect("second approval writer"),
    ];
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.state() == RuntimeReceiptState::Completed)
            .count(),
        1
    );
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.state() == RuntimeReceiptState::Denied)
            .count(),
        1
    );
    assert!(receipts.iter().any(|receipt| {
        receipt
            .error_projection()
            .is_some_and(|error| error.code == RuntimeErrorCode::InvalidRequest)
    }));

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn acquire_idempotency_recovers_its_durable_terminal_without_a_connection_cache() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::acquire_lease("node.a", ids.mint_holder_id().expect("holder id")),
    )
    .expect("runtime request");
    let connection = ConnectionId::new(99).expect("connection id");

    let first = host
        .process_request_for_test(&request, connection)
        .expect("first acquire");
    let repeated = host
        .process_request_for_test(&request, connection)
        .expect("repeated acquire");

    assert_eq!(repeated, first);
    assert!(first.terminal().is_some());
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);

    let query = RuntimeRequest::new(
        ids.mint_request_id().expect("query request id"),
        ids.mint_correlation_id().expect("query correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::QueryEvents {
            query: EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
            profile: ProjectionProfile::Forensic,
        },
    )
    .expect("query request");
    let receipt = host
        .process_request_for_test(&query, connection)
        .expect("query receipt");
    let RuntimeResult::Events { events } = receipt.result().expect("events result") else {
        panic!("expected event projection");
    };
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        vec![
            EventType::LeaseRequested,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseGranted,
        ]
    );
    host.close().expect("close host");
}

#[test]
fn renew_and_release_idempotency_survive_connection_cache_loss() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection_id = ConnectionId::new(99).expect("connection id");
    let acquire = runtime_request(
        &ids,
        RuntimeOperation::acquire_lease("node.a", ids.mint_holder_id().expect("holder id")),
    );
    let receipt = host
        .process_request_for_test(&acquire, connection_id)
        .expect("acquire");
    let RuntimeResult::LeaseGranted { token } = receipt.result().expect("lease result") else {
        panic!("expected lease grant");
    };

    let renew = runtime_request(
        &ids,
        RuntimeOperation::RenewLease {
            token: token.clone(),
        },
    );
    let first_renew = host
        .process_request_for_test(&renew, connection_id)
        .expect("first renew");
    let repeated_renew = host
        .process_request_for_test(&renew, connection_id)
        .expect("repeated renew");
    assert_eq!(repeated_renew, first_renew);
    assert_eq!(
        event_types_for_request(&host, &ids, connection_id, renew.request_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseRenewed,
        ]
    );
    let RuntimeResult::LeaseRenewed { token } = first_renew.result().expect("renew result") else {
        panic!("expected renewed lease");
    };

    let release = runtime_request(
        &ids,
        RuntimeOperation::ReleaseLease {
            token: token.clone(),
        },
    );
    let first_release = host
        .process_request_for_test(&release, connection_id)
        .expect("first release");
    let repeated_release = host
        .process_request_for_test(&release, connection_id)
        .expect("repeated release");
    assert_eq!(repeated_release, first_release);
    assert_eq!(
        event_types_for_request(&host, &ids, connection_id, release.request_id()),
        vec![
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseReleased,
        ]
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    host.close().expect("close host");
}

#[test]
fn second_owner_is_rejected_and_clean_restart_gets_a_new_epoch() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let first = host_with_state(&root, "node.a", Arc::clone(&state));
    assert!(root.path().join(RUNTIME_INFO_FILE).is_file());
    let first_epoch = first.runtime_info().owner_epoch();
    let error = match RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::clone(&state),
        )),
    ) {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("second owner must fail");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "owner_conflict");
    assert_eq!(error.projection().code, RuntimeErrorCode::OwnerConflict);
    first.close().expect("close first host");
    assert!(!root.path().join(RUNTIME_INFO_FILE).exists());

    let second = host_with_state(&root, "node.a", state);
    assert_ne!(second.runtime_info().owner_epoch(), first_epoch);
    second.close().expect("close second host");
}

#[test]
fn owner_journal_recovers_only_an_incomplete_final_record() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "node.a", Arc::clone(&state))
        .close()
        .expect("close initial host");
    let owner_path = root.path().join(crate::owner::OWNER_FILE_NAME);
    OpenOptions::new()
        .append(true)
        .open(&owner_path)
        .expect("open owner journal")
        .write_all(br#"{"incomplete"#)
        .expect("append incomplete tail");

    let recovered = host_with_state(&root, "node.a", state);
    recovered.close().expect("close recovered host");
    let content = std::fs::read(&owner_path).expect("read owner journal");
    assert!(content.ends_with(b"\n"));
    assert!(!content.windows(10).any(|window| window == b"incomplete"));
}

#[test]
fn complete_owner_journal_corruption_is_fatal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "node.a", Arc::clone(&state))
        .close()
        .expect("close initial host");
    let owner_path = root.path().join(crate::owner::OWNER_FILE_NAME);
    OpenOptions::new()
        .append(true)
        .open(owner_path)
        .expect("open owner journal")
        .write_all(b"not-json\n")
        .expect("append corruption");
    let result = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("node.a", instance_id(), state)),
    );
    let error = match result {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("corrupt owner journal must fail");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "owner_record_invalid");
    assert!(error.is_fatal());
}

#[test]
fn connection_drop_revokes_lease_without_opening_the_lazy_backend() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let _ = client.acquire("node.a");
    drop(client);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);

    let mut replacement = TestClient::connect(&host);
    let (_, token) = replacement.acquire("node.a");
    let release = replacement.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        replacement.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(replacement);
    host.close().expect("close host");
    assert_eq!(state.close_count.load(Ordering::Acquire), 0);
}

#[test]
fn every_fencing_field_is_checked_before_backend_use() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let stale_epoch = LeaseToken::new(
        *ids.mint_owner_epoch().expect("owner epoch").transport(),
        token.lease_id(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("stale epoch token");
    assert_input_denied(&mut client, stale_epoch, RuntimeErrorCode::StaleOwnerEpoch);

    let wrong_lease = LeaseToken::new(
        token.owner_epoch(),
        *ids.mint_lease_id().expect("lease id").transport(),
        token.instance_id(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong lease token");
    assert_input_denied(&mut client, wrong_lease, RuntimeErrorCode::LeaseMismatch);

    let wrong_instance = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        *ids.mint_instance_id().expect("instance id").transport(),
        token.holder_id(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong instance token");
    assert_input_denied(
        &mut client,
        wrong_instance,
        RuntimeErrorCode::InstanceMismatch,
    );

    let wrong_holder = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        token.instance_id(),
        *ids.mint_holder_id().expect("holder id").transport(),
        token.expires_at_monotonic_ms(),
    )
    .expect("wrong holder token");
    assert_input_denied(&mut client, wrong_holder, RuntimeErrorCode::HolderMismatch);

    let mut intruder = TestClient::connect(&host);
    let cross_connection = intruder.request(RuntimeOperation::Input {
        token: token.clone(),
        action: InputAction::Tap { x: 10, y: 20 },
    });
    let receipt = intruder.send(&cross_connection);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::ConnectionMismatch
    );
    drop(intruder);
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);

    let release = client.request(RuntimeOperation::ReleaseLease { token });
    client.send(&release);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn backend_failure_is_visible_and_revokes_the_guard() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_input.store(true, Ordering::Release);
    let host = host_with_state(&root, "node.a", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("node.a");
    let input = client.request(RuntimeOperation::Input {
        token,
        action: InputAction::Reset,
    });
    let receipt = client.send(&input);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed);
    assert_eq!(
        receipt.error_projection().expect("failure").code,
        RuntimeErrorCode::BackendOperationFailed
    );
    wait_until(Duration::from_secs(2), || {
        state.close_count.load(Ordering::Acquire) == 1
    });
    drop(client);
    assert!(host.fatal_error().expect("health").is_none());
    host.close().expect("close host");
}

#[test]
fn expired_unopened_lease_is_reclaimed_before_a_new_grant() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let provider = Arc::new(FakeProvider::one(
        "node.a",
        instance_id(),
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(
        config(&root).with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 100,
            takeover_cooldown_ms: 200,
            lease_ttl_ms: 1_000,
            ..SchedulerConfig::default()
        }),
        provider,
    )
    .expect("runtime host");
    let mut first = TestClient::connect(&host);
    let _ = first.acquire("node.a");
    thread::sleep(Duration::from_millis(1_100));
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    let mut second = TestClient::connect(&host);
    let (_, token) = second.acquire("node.a");
    let release = second.request(RuntimeOperation::ReleaseLease { token });
    second.send(&release);
    drop(first);
    drop(second);
    host.close().expect("close host");
}

#[test]
fn runtime_fact_store_shares_server_facts_invalidates_and_recovers_from_ledger() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let registered_id = instance_id();
    let provider = Arc::new(FakeProvider::one(
        POLICY_INSTANCE_ALIAS,
        registered_id,
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(config(&root), provider).expect("runtime host");
    let server_record = stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:server-theme",
        vec![EventType::PolicyPlanningSignalObserved],
    );
    let instance_record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "inventory.items",
        ContractFactValue::RecordList(Vec::new()),
        "snapshot:instance-inventory",
        Vec::new(),
    );
    let published = host
        .publish_fact(server_record.clone())
        .expect("publish server fact");
    assert_eq!(
        host.publish_fact(server_record.clone())
            .expect("idempotent fact publication"),
        published
    );
    host.publish_fact(instance_record)
        .expect("publish instance fact");

    let primary = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("primary snapshot");
    let peer = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: "fixture-instance-b".to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("peer snapshot");
    assert_eq!(primary.records.len(), 2);
    assert_eq!(peer.records.len(), 1);

    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:fact-invalidation".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::GoalMissed,
        fact_code: "goal.fixture.missed".to_owned(),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 1,
        detection_budget: None,
    })
    .expect("record invalidating event");
    let after = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("snapshot after invalidation");
    assert_eq!(after.records.len(), 1);
    assert_eq!(after.records[0].key, "inventory.items");
    let stale = host
        .publish_fact(server_record)
        .expect_err("invalidated source snapshot must not be resurrected");
    assert_eq!(stale.code(), "fact_source_snapshot_invalidated");
    assert!(!stale.is_fatal());

    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            }
        )
        .len(),
        2
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            state,
        )),
    )
    .expect("reopen runtime host");
    let recovered = reopened
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("recovered snapshot");
    assert_eq!(recovered.records.len(), 1);
    assert_eq!(recovered.records[0].key, "inventory.items");
    reopened.close().expect("close reopened host");
}

#[test]
fn information_planning_signals_are_queryable_and_subscription_pages_are_lossless() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    for index in 0..5_u64 {
        host.record_policy_planning_signal(PolicyPlanningSignalEventData {
            signal_id: format!("signal:projection-{index}"),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: None,
            kind: PolicyPlanningSignalKind::GoalMissed,
            fact_code: format!("goal.fixture.projection-{index}"),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS + index,
            detection_budget: None,
        })
        .expect("record planning signal");
    }

    let query = EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        minimum_severity: Some(EventSeverity::Info),
        ..EventQuery::default()
    };
    let mut client = TestClient::connect(&host);
    let expected = projected_events(&mut client, query.clone())
        .into_iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 5);

    let mut cursor = actingcommand_contract::SubscriptionCursor::default();
    let mut observed = Vec::new();
    for _ in 0..8 {
        let subscription = actingcommand_contract::RuntimeSubscriptionRequest::new(
            query.clone(),
            ProjectionProfile::Forensic,
            cursor,
            100,
            2,
        )
        .expect("subscription request");
        let request = client.request(RuntimeOperation::SubscribeEvents {
            request: subscription,
        });
        let receipt = client.send(&request);
        let RuntimeResult::EventBatch { batch } = receipt.result().expect("event batch") else {
            panic!("expected event batch")
        };
        assert!(!batch.timed_out(), "planning signal page must not be empty");
        assert!(batch.events().len() <= 2);
        assert!(batch.events().iter().all(|event| {
            event.event_type == EventType::PolicyPlanningSignalObserved
                && event.severity == EventSeverity::Info
        }));
        observed.extend(batch.events().iter().map(|event| event.sequence));
        cursor = batch.next_cursor();
        if observed.len() == expected.len() {
            break;
        }
    }

    assert_eq!(observed, expected);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn policy_evaluation_consumes_runtime_owned_fact_projection() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("tasks fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "server", "server_id": "fixture-server-a"},
        "fact_key": "env.ui_theme",
        "comparison": "eq",
        "value": {"type": "string", "value": "Neutral"},
        "max_age_ms": 60_000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate policy catalog");
    host.publish_fact(stored_fact(
        FactScope::Server {
            server_id: "fixture-server-a".to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:policy-theme",
        Vec::new(),
    ))
    .expect("publish policy fact");

    let cycle = host
        .evaluate_policy_cycle(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("evaluate fact-backed policy");
    let evaluation = cycle.evaluation.expect("policy evaluation");
    assert_eq!(evaluation.dispatch_intents.len(), 1);
    assert!(
        evaluation.dispatch_intents[0]
            .fact_snapshot_id
            .starts_with("snapshot:policy-fact:")
    );
    host.close().expect("close host");
}

#[test]
fn fact_snapshot_catches_up_with_critical_ledger_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:critical-event",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");

    let snapshot = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("synchronized fact snapshot");
    assert!(snapshot.records.is_empty());

    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_startup_materializes_a_missed_critical_fact_invalidation() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    host.publish_fact(stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ui_theme",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:restart-critical-event",
        vec![EventType::CatalogActivated],
    ))
    .expect("publish fact");
    host.activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    host.close().expect("close without reading facts");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    let snapshot = reopened
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
        })
        .expect("recovered snapshot");
    assert!(snapshot.records.is_empty());
    let mut client = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::FactInvalidated),
                ..EventQuery::default()
            }
        )
        .len(),
        1
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn policy_cadence_is_explicit_and_clock_jumps_force_full_recompute() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let facts = policy_facts();
    let resources = policy_resources();

    let startup = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("startup policy cycle");
    assert_eq!(startup.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        startup.directive.reason,
        PolicyRecomputeReason::StartupOrRecovery
    );
    assert!(startup.evaluation.is_some());
    let startup_measurement = startup.measurement.expect("startup measurement");
    assert_eq!(
        startup_measurement.requested_recompute,
        PolicyRecomputeKind::Full
    );
    assert_eq!(
        startup_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    assert!(startup_measurement.cost.work_units > 0);

    let cooldown = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 100,
            },
            7,
            PolicyTrigger::ResourcesChanged,
        )
        .expect("cooldown policy cycle");
    assert_eq!(cooldown.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(cooldown.directive.reason, PolicyRecomputeReason::Cooldown);
    assert!(cooldown.evaluation.is_none());
    assert!(cooldown.measurement.is_none());

    let incremental = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 1_100,
                monotonic_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("incremental policy cycle");
    assert_eq!(incremental.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(incremental.directive.reason, PolicyRecomputeReason::Event);
    let incremental_measurement = incremental.measurement.expect("incremental measurement");
    assert_eq!(
        incremental_measurement.requested_recompute,
        PolicyRecomputeKind::Incremental
    );
    assert_eq!(
        incremental_measurement.execution,
        PolicyEvaluationExecution::FullCatalogScan
    );
    assert_eq!(incremental_measurement.cost, startup_measurement.cost);
    assert!(
        incremental_measurement.sampled_at_monotonic_ms
            >= startup_measurement.sampled_at_monotonic_ms
    );

    let clock_jump = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 7_000,
            },
            7,
            PolicyTrigger::ClockObserved {
                previous_unix_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
        )
        .expect("clock-jump policy cycle");
    assert_eq!(clock_jump.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        clock_jump.directive.reason,
        PolicyRecomputeReason::ClockJump
    );

    let reconciliation = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 67_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 67_000,
            },
            7,
            PolicyTrigger::Reconciliation,
        )
        .expect("reconciliation policy cycle");
    assert_eq!(reconciliation.directive.kind, PolicyRecomputeKind::Full);
    assert_eq!(
        reconciliation.directive.reason,
        PolicyRecomputeReason::Reconciliation
    );
    host.close().expect("close host");
}

#[test]
fn catalog_cas_conflict_preserves_nonfatal_identity_and_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let first = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let second = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");

    let error = host
        .activate_policy_catalog_with_expected_for_test(&policy_sources(3), first)
        .expect_err("stale compare-and-swap must fail");
    assert_eq!(error.code(), "catalog_active_generation_changed");
    assert_eq!(error.operation(), "switch_active_catalog");
    assert!(!error.is_fatal());
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("active generation"),
        second
    );

    let mut client = TestClient::connect(&host);
    let failures = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogTransitionFailed),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) =
        &failures.last().expect("catalog transition failure").payload
    else {
        panic!("expected full catalog transition failure payload");
    };
    assert_eq!(
        payload.effect_disposition(),
        Some(EffectDisposition::NotPerformed)
    );

    host.activate_policy_catalog(&policy_sources(3))
        .expect("runtime remains usable after stale compare-and-swap");
    drop(client);
    host.close().expect("close host");
}

#[test]
fn policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let first_catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let forged_approval = policy_context(&host, &intent);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &forged_approval)
        .expect_err("caller-supplied approval IDs must not grant authority");
    assert_eq!(error.code(), "policy_approval_fact_missing");
    record_policy_approval(&host, &intent);

    let mut tampered_intent = intent.clone();
    tampered_intent.decision_id = "decision:tampered".to_owned();
    tampered_intent.reason_chain_id = "reason:tampered".to_owned();
    tampered_intent.approval_refs.clear();
    let mut tampered_reasons = reasons.clone();
    tampered_reasons.id = "reason:tampered".to_owned();
    tampered_reasons.decision_id = "decision:tampered".to_owned();
    let error = host
        .admit_policy_dispatch(
            &tampered_intent,
            &tampered_reasons,
            &policy_context(&host, &tampered_intent),
        )
        .expect_err("catalog approval requirements cannot be stripped");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    let (_, approved_intent, approved_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        8,
    );

    let admission = host
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&host, &approved_intent),
        )
        .expect("policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    let mut stale_context = policy_context(&host, &approved_intent);
    stale_context.now_unix_ms = POLICY_NOW_UNIX_MS + 60_000;
    let replay = host
        .admit_policy_dispatch(&approved_intent, &approved_reasons, &stale_context)
        .expect("exact replay is suppressed before mutable-state revalidation");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let after = projected_events(&mut client, EventQuery::default());
    assert_eq!(before.len(), after.len());
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
            .count(),
        2
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
            .count(),
        1
    );

    let second_catalog = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    assert_ne!(first_catalog.catalog_hash(), second_catalog.catalog_hash());
    assert_eq!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut old_new_intent = intent.clone();
    old_new_intent.decision_id = "decision:fixture-b".to_owned();
    old_new_intent.reason_chain_id = "reason:fixture-b".to_owned();
    let mut old_new_reasons = reasons.clone();
    old_new_reasons.id = "reason:fixture-b".to_owned();
    old_new_reasons.decision_id = "decision:fixture-b".to_owned();
    let error = host
        .admit_policy_dispatch(
            &old_new_intent,
            &old_new_reasons,
            &policy_context(&host, &old_new_intent),
        )
        .expect_err("new admission cannot use the old catalog");
    assert_eq!(error.code(), "policy_decision_not_host_evaluated");

    host.complete_policy_dispatch(&approved_intent.decision_id)
        .expect("complete policy dispatch");
    assert!(
        host.pinned_policy_catalog(&approved_intent.decision_id)
            .expect("pinned catalog")
            .is_none()
    );
    let rolled_back = host
        .rollback_policy_catalog(first_catalog.catalog_hash())
        .expect("rollback policy catalog");
    assert_eq!(rolled_back, first_catalog);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogActivated)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::CatalogRolledBack)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchCompleted)
    );
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );
    let replay = reopened
        .admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&reopened, &approved_intent),
        )
        .expect("replay after restart");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close reopened host");
}

#[test]
fn policy_budget_recovery_keeps_the_window_count_across_runtime_restarts() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();

    for index in 0_u64..4 {
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("budget runtime host");
        if index == 0 {
            host.activate_policy_catalog(&budget_policy_sources(1))
                .expect("activate budget catalog");
        }
        let (_, intent, reasons) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Recovery,
            POLICY_NOW_UNIX_MS + index * 600_000,
            100 + index,
        );
        if index == 0 {
            record_policy_approval(&host, &intent);
        }
        let admission = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .expect("budget admission");
        let PolicyDispatchAdmission::Granted { admission, .. } = admission else {
            panic!("expected budget admission")
        };
        assert_eq!(admission.budget.task_window_used, index as u32 + 1);
        host.record_policy_dispatch_outcome(
            &intent.decision_id,
            admission.activity.admitted_at_unix_ms + 75_000,
            &PolicyExecutionInput::Succeeded,
        )
        .expect("record bounded execution");
        host.close().expect("close budget runtime host");
    }

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen exhausted budget runtime");
    let (_, intent, reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Recovery,
        POLICY_NOW_UNIX_MS + 2_400_000,
        104,
    );
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("fifth window admission must remain rejected after restart");
    assert_eq!(error.code(), "policy_budget_exhausted");
    host.close().expect("close exhausted budget runtime");
}

#[test]
fn accelerated_48h_replay_consumes_runtime_owned_counts_and_runtime_budget() {
    const HOUR_MS: u64 = 3_600_000;
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let start_unix_ms = POLICY_NOW_UNIX_MS + 12 * HOUR_MS;

    for day in 0_u64..2 {
        for iteration in 0_u64..4 {
            let host = RuntimeHost::start(
                config(&root),
                Arc::new(FakeProvider::one(
                    POLICY_INSTANCE_ALIAS,
                    registered_id,
                    Arc::new(FakeState::default()),
                )),
            )
            .expect("accelerated budget runtime");
            if day == 0 && iteration == 0 {
                host.activate_policy_catalog(&budget_policy_sources(1))
                    .expect("activate accelerated budget catalog");
            }
            let unix_ms = start_unix_ms + (day * 24 + iteration * 6) * HOUR_MS;
            let (_, intent, reasons) = evaluated_policy_dispatch_at(
                &host,
                if day == 0 && iteration == 0 {
                    PolicyTrigger::FactsChanged
                } else {
                    PolicyTrigger::Reconciliation
                },
                unix_ms,
                200 + day * 10 + iteration,
            );
            if day == 0 && iteration == 0 {
                record_policy_approval(&host, &intent);
            }
            let admission = host
                .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
                .expect("accelerated budget admission");
            let PolicyDispatchAdmission::Granted { admission, .. } = admission else {
                panic!("expected accelerated budget admission")
            };
            assert_eq!(admission.budget.task_daily_used, iteration as u32 + 1);
            assert_eq!(admission.budget.task_window_used, iteration as u32 + 1);
            assert_eq!(
                admission.budget.task_runtime_reserved_ms,
                iteration * 75_000 + 60_000
            );
            host.record_policy_dispatch_outcome(
                &intent.decision_id,
                admission.activity.admitted_at_unix_ms + 75_000,
                &PolicyExecutionInput::Succeeded,
            )
            .expect("accelerated bounded execution");
            host.close().expect("close accelerated budget iteration");
        }

        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("accelerated exhausted budget runtime");
        let final_hour = day * 24 + 23;
        let (_, intent, reasons) = evaluated_policy_dispatch_at(
            &host,
            PolicyTrigger::Reconciliation,
            start_unix_ms + final_hour * HOUR_MS,
            209 + day * 10,
        );
        let error = host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
            .expect_err("fifth daily/window execution must exhaust the production budget");
        assert_eq!(error.code(), "policy_budget_exhausted");
        host.close()
            .expect("close accelerated exhausted budget runtime");
    }
}

#[test]
fn detection_quota_is_persistent_informational_and_never_starves_ordinary_work() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    host.activate_policy_catalog(&detection_policy_sources(1))
        .expect("activate detection catalog");

    let ordinary = host
        .evaluate_policy_cycle(
            &detection_policy_facts(true, "snapshot:detection-ordinary"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            1,
            PolicyTrigger::FactsChanged,
        )
        .expect("ordinary-first detection cycle");
    assert_eq!(ordinary.pending_dispatch_intents.len(), 1);
    assert_eq!(
        ordinary.pending_dispatch_intents[0].task_id,
        "fixture.observe"
    );
    assert!(ordinary.detection_planning_signals.is_empty());

    for index in 1_u64..=2 {
        let cycle = host
            .evaluate_policy_cycle(
                &detection_policy_facts(false, &format!("snapshot:detection-reserved-{index}")),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + index * 2_000,
                    monotonic_ms: POLICY_NOW_UNIX_MS + index * 2_000,
                },
                index + 1,
                PolicyTrigger::FactsChanged,
            )
            .expect("reserve detection quota");
        assert!(cycle.pending_dispatch_intents.is_empty());
        assert_eq!(cycle.detection_planning_signals.len(), 1);
        let signal = &cycle.detection_planning_signals[0];
        assert_eq!(signal.kind, PolicyPlanningSignalKind::DetectionReserved);
        let budget = signal.detection_budget.as_ref().expect("detection budget");
        assert_eq!(
            budget.dispatch_used,
            u32::try_from(index).expect("small detection index")
        );
        assert_eq!(budget.runtime_reserved_ms, index * 10_000);
        if index == 1 {
            let mut forged = signal.clone();
            forged.signal_id = "signal:detection:caller-forged".to_owned();
            let error = host
                .record_policy_planning_signal(forged)
                .expect_err("callers cannot forge runtime-owned detection quota events");
            assert_eq!(error.code(), "policy_detection_signal_runtime_owned");
            assert!(!error.is_fatal());
        }
    }

    host.activate_policy_catalog(&detection_policy_sources(2))
        .expect("upgrade detection catalog without resetting quota");

    let exhausted = host
        .evaluate_policy_cycle(
            &detection_policy_facts(false, "snapshot:detection-exhausted"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 6_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 6_000,
            },
            4,
            PolicyTrigger::FactsChanged,
        )
        .expect("exhaust detection quota");
    assert!(exhausted.pending_dispatch_intents.is_empty());
    assert_eq!(exhausted.detection_planning_signals.len(), 1);
    assert_eq!(
        exhausted.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );

    let ordinary_after_exhaustion = host
        .evaluate_policy_cycle(
            &detection_policy_facts(true, "snapshot:detection-ordinary-after-exhaustion"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 7_000,
            },
            5,
            PolicyTrigger::FactsChanged,
        )
        .expect("ordinary work after detection quota exhaustion");
    assert_eq!(ordinary_after_exhaustion.pending_dispatch_intents.len(), 1);
    assert_eq!(
        ordinary_after_exhaustion.pending_dispatch_intents[0].task_id,
        "fixture.observe"
    );
    assert!(
        ordinary_after_exhaustion
            .detection_planning_signals
            .is_empty()
    );

    let mut client = TestClient::connect(&host);
    let signals = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signals.len(), 3);
    assert!(
        signals
            .iter()
            .all(|event| event.severity == EventSeverity::Info)
    );
    assert_eq!(
        signals
            .iter()
            .filter(|event| {
                matches!(
                    &event.payload,
                    ProjectionPayload::Full(payload)
                        if matches!(
                            payload.as_ref(),
                            EventPayload::Policy(PolicyPayload::PlanningSignalObserved(signal))
                                if signal.kind()
                                    == PolicyPlanningSignalKind::DetectionQuotaExhausted
                        )
                )
            })
            .count(),
        1
    );
    drop(client);
    let mut dispatch_client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut dispatch_client,
            EventQuery {
                event_type: Some(EventType::PolicyDispatchIntent),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "detection reservations must not be recorded as ordinary dispatch success"
    );
    drop(dispatch_client);
    host.close().expect("close detection runtime");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen detection runtime");
    let recovered = reopened
        .evaluate_policy_cycle(
            &detection_policy_facts(false, "snapshot:detection-after-restart"),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 9_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 9_000,
            },
            6,
            PolicyTrigger::Recovery,
        )
        .expect("recover exhausted detection quota");
    assert_eq!(recovered.detection_planning_signals.len(), 1);
    assert_eq!(
        recovered.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    assert!(recovered.pending_dispatch_intents.is_empty());
    reopened.close().expect("close recovered detection runtime");
}

#[test]
fn approval_decision_is_authoritative_target_bound_and_revocable() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    let forged = policy_context(&host, &intent);
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &forged)
            .expect_err("caller approval set is not authoritative")
            .code(),
        "policy_approval_fact_missing"
    );

    record_policy_approval(&host, &intent);
    let (_, approved_intent, approved_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 60_000,
        8,
    );
    assert!(matches!(
        host.admit_policy_dispatch(
            &approved_intent,
            &approved_reasons,
            &policy_context(&host, &approved_intent),
        )
        .expect("approved dispatch"),
        PolicyDispatchAdmission::Granted { .. }
    ));

    let mut client = TestClient::connect(&host);
    client.authenticate_governance();
    let conflicting = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:fixture-a",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "f".repeat(64)),
                catalog_version: 1,
            },
            "user_confirmed",
        )
        .expect("conflicting approval"),
    });
    assert_eq!(
        client.send(&conflicting).state(),
        RuntimeReceiptState::Denied
    );
    drop(client);

    host.complete_policy_dispatch(&approved_intent.decision_id)
        .expect("complete approved dispatch");
    record_policy_approval_disposition(&host, &approved_intent, ApprovalDisposition::Revoked);
    let (_, after_revoke, after_revoke_reasons) = evaluated_policy_dispatch_at(
        &host,
        PolicyTrigger::Reconciliation,
        POLICY_NOW_UNIX_MS + 120_000,
        9,
    );
    assert_eq!(
        host.admit_policy_dispatch(
            &after_revoke,
            &after_revoke_reasons,
            &policy_context(&host, &after_revoke),
        )
        .expect_err("revoked approval must not authorize a new dispatch")
        .code(),
        "policy_approval_fact_missing"
    );

    let mut client = TestClient::connect(&host);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 2);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn approval_history_compacts_without_losing_durable_target_identity() {
    let root = TempDir::new().expect("tempdir");
    let registered_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let target = ApprovalTarget::Catalog {
        catalog_hash: format!("sha256:{}", "a".repeat(64)),
        catalog_version: 1,
    };
    let mut client = TestClient::connect(&host);
    client.authenticate_governance();
    for index in 0..257 {
        let approval_id = format!("approval:history-{index}");
        let request = client.governance_request(RuntimeOperation::RecordApprovalDecision {
            decision: ApprovalDecisionRecord::new(
                approval_id,
                ApprovalDisposition::Rejected,
                target.clone(),
                "history_compaction",
            )
            .expect("approval decision"),
        });
        assert_eq!(
            client.send(&request).state(),
            RuntimeReceiptState::Completed
        );
    }
    drop(client);

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ProjectInterface {
        request: ProjectInterfaceRequest::current(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProjectInterface { response } = receipt.result().expect("projection result")
    else {
        panic!("expected project interface response")
    };
    assert_eq!(response.snapshot().approvals.len(), 256);
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    let mut client = TestClient::connect(&reopened);
    client.authenticate_governance();
    let conflict = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: ApprovalDecisionRecord::new(
            "approval:history-0",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "b".repeat(64)),
                catalog_version: 1,
            },
            "conflicting_target",
        )
        .expect("conflicting approval"),
    });
    assert_eq!(client.send(&conflict).state(), RuntimeReceiptState::Denied);
    assert!(reopened.fatal_error().expect("runtime health").is_none());
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn governance_authority_is_capability_authenticated_and_connection_bound() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let target = ApprovalTarget::Catalog {
        catalog_hash: format!("sha256:{}", "a".repeat(64)),
        catalog_version: 1,
    };
    let approval = |disposition| {
        ApprovalDecisionRecord::new(
            "approval:governance-boundary",
            disposition,
            target.clone(),
            "user_confirmed",
        )
        .expect("approval decision")
    };

    let mut client = TestClient::connect(&host);
    let unauthenticated = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Approved),
    });
    let receipt = client.send(&unauthenticated);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    let wrong_capability = client.governance_request(RuntimeOperation::AuthenticateGovernance {
        capability: "wrong-governance-capability-value".to_owned(),
    });
    let receipt = client.send(&wrong_capability);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    client.authenticate_governance();
    let approved = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Approved),
    });
    assert_eq!(
        client.send(&approved).state(),
        RuntimeReceiptState::Completed
    );

    let mut other = TestClient::connect(&host);
    let forged_revocation = other.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Revoked),
    });
    let receipt = other.send(&forged_revocation);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::InvalidRequest
    );

    let revoked = client.governance_request(RuntimeOperation::RecordApprovalDecision {
        decision: approval(ApprovalDisposition::Revoked),
    });
    assert_eq!(
        client.send(&revoked).state(),
        RuntimeReceiptState::Completed
    );

    let events = projected_events(
        &mut other,
        EventQuery {
            event_type: Some(EventType::ApprovalDecision),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| {
        event.origin.source() == EventSource::Ui
            && event.origin.module() == OriginModule::Governance
            && event.origin.actor() == EventActor::User
    }));
    drop(client);
    drop(other);
    host.close().expect("close host");
}

#[test]
fn historical_agent_approval_poisoning_is_fatal_during_recovery() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.append_approval_event_for_test(
        EventSource::Adapter,
        EventActor::Agent,
        ApprovalDecisionRecord::new(
            "approval:historical-agent",
            ApprovalDisposition::Approved,
            ApprovalTarget::Catalog {
                catalog_hash: format!("sha256:{}", "a".repeat(64)),
                catalog_version: 1,
            },
            "agent_claimed_approval",
        )
        .expect("approval decision"),
    )
    .expect("historical malicious approval fixture");
    host.close().expect("close host");

    let error = match RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    ) {
        Ok(host) => {
            host.close().expect("close unexpected host");
            panic!("historical Agent approval must prevent recovery")
        }
        Err(error) => error,
    };
    assert!(error.is_fatal());
    assert_eq!(error.code(), "approval_projection_origin_invalid");
}

#[test]
fn policy_admission_rejects_stale_and_tampered_trusted_context() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("tasks fixture");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": "env.ephemeral",
        "comparison": "eq",
        "value": {"type": "string", "value": "Neutral"},
        "max_age_ms": 20
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("tasks bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog");
    let mut ephemeral = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "env.ephemeral",
        ContractFactValue::String("Neutral".to_owned()),
        "snapshot:ephemeral",
        Vec::new(),
    );
    ephemeral.expires_at_unix_ms = Some(POLICY_NOW_UNIX_MS + 20);
    ephemeral.ttl_policy = Some(FactTtlPolicy {
        minimum_ms: 1,
        maximum_ms: 100,
        source: FactTtlSource::DetectorContract,
    });
    host.publish_fact(ephemeral)
        .expect("publish ephemeral fact");

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let mut forged_decision = intent.clone();
    forged_decision.decision_id = "decision:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &forged_decision,
            &reasons,
            &policy_context(&host, &forged_decision),
        )
        .expect_err("forged decision identity must fail")
        .code(),
        "policy_decision_not_host_evaluated"
    );

    let mut wrong_profile = intent.clone();
    wrong_profile.prerequisites.activity_profile_id = "profile:forged".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_profile,
            &reasons,
            &policy_context(&host, &wrong_profile),
        )
        .expect_err("caller-selected profile must fail")
        .code(),
        "policy_trusted_context_mismatch"
    );

    thread::sleep(Duration::from_millis(30));
    let mut forged_time = policy_context(&host, &intent);
    forged_time.now_unix_ms = 1;
    assert_eq!(
        host.admit_policy_dispatch(&intent, &reasons, &forged_time)
            .expect_err("expired fact must fail admission")
            .code(),
        "policy_facts_stale"
    );
    host.close().expect("close host");
}

#[test]
fn game_and_server_scope_changes_cannot_reuse_a_trusted_decision_identity() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    let (_, trusted, trusted_reasons) =
        evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let mut wrong_instance = trusted.clone();
    wrong_instance.instance_id = "fixture-instance-b".to_owned();
    assert_eq!(
        host.admit_policy_dispatch(
            &wrong_instance,
            &trusted_reasons,
            &policy_context(&host, &wrong_instance),
        )
        .expect_err("instance scope cannot be widened by the caller")
        .code(),
        "policy_trusted_context_mismatch"
    );

    for (index, mut facts) in [policy_facts(), policy_facts()].into_iter().enumerate() {
        if index == 0 {
            facts.instances[0].server_id = "fixture-server-b".to_owned();
        } else {
            facts.instances[0].game_id = "fixture-game-b".to_owned();
        }
        let cycle = host
            .evaluate_policy_cycle(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                    monotonic_ms: POLICY_NOW_UNIX_MS + 60_000 * (index as u64 + 1),
                },
                8 + index as u64,
                PolicyTrigger::Reconciliation,
            )
            .expect("scope-changed policy evaluation");
        let changed = cycle
            .evaluation
            .expect("scope-changed evaluation")
            .dispatch_intents
            .into_iter()
            .next()
            .expect("scope-changed intent");
        assert_ne!(changed.fact_snapshot_id, trusted.fact_snapshot_id);
        assert_ne!(changed.decision_id, trusted.decision_id);

        let mut mixed = changed;
        mixed.decision_id = trusted.decision_id.clone();
        mixed.reason_chain_id = trusted.reason_chain_id.clone();
        assert_eq!(
            host.admit_policy_dispatch(&mixed, &trusted_reasons, &policy_context(&host, &mixed),)
                .expect_err("scope changes cannot reuse an old decision identity")
                .code(),
            "policy_trusted_context_mismatch"
        );
    }
    host.close().expect("close host");
}

#[test]
fn measured_contention_gates_deadline_dispatch_and_records_the_conflict() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    let mut sources = policy_sources(1);
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("activity fixture");
    activity["profiles"][0]["goals"][0]["deadline_unix_ms"] = serde_json::json!(POLICY_NOW_UNIX_MS);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    host.activate_policy_catalog(&sources)
        .expect("activate catalog");
    host.observe_performance_control_for_test(PerformanceControlObservation {
        observed_at_unix_ms: POLICY_NOW_UNIX_MS - 2_000,
        host_responsiveness_basis_points: Some(7_000),
        third_party_pressure_basis_points: Some(0),
        foreground_fullscreen: false,
    })
    .expect("first contention sample");
    host.observe_performance_control_for_test(PerformanceControlObservation {
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        host_responsiveness_basis_points: Some(7_000),
        third_party_pressure_basis_points: Some(0),
        foreground_fullscreen: false,
    })
    .expect("second contention sample");
    assert_eq!(
        host.performance_control_directive(POLICY_INSTANCE_ALIAS)
            .expect("directive")
            .level,
        PerformanceControlLevel::DispatchPaused
    );

    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.prerequisites.urgency_milli, 1_000);
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect_err("deadline must not bypass a measured contention gate");
    assert_eq!(error.code(), "performance_capacity_deadline_conflict");

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PerformanceBalanceChanged)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == EventType::PolicyDispatchRejected)
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == EventType::LeaseGranted)
    );
    host.close().expect("close host");
}

#[test]
fn policy_failure_activity_and_planning_facts_recover_without_duplicate_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted {
        admission: budget_record,
        ..
    } = admission
    else {
        panic!("expected granted policy admission")
    };
    assert_eq!(budget_record.budget.task_daily_used, 1);
    assert_eq!(budget_record.budget.activity_window_used, 1);
    assert!(budget_record.activity.seed > 0);

    let signals = [
        (
            "signal:goal-missed-a",
            PolicyPlanningSignalKind::GoalMissed,
            "goal.primary.missed",
        ),
        (
            "signal:feasibility-red-a",
            PolicyPlanningSignalKind::FeasibilityRed,
            "goal.primary.feasibility_red",
        ),
        (
            "signal:drift-predicted-a",
            PolicyPlanningSignalKind::DriftPredicted,
            "goal.primary.drift_predicted",
        ),
    ]
    .map(
        |(signal_id, kind, fact_code)| PolicyPlanningSignalEventData {
            signal_id: signal_id.to_owned(),
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            task_id: Some(intent.task_id.clone()),
            kind,
            fact_code: fact_code.to_owned(),
            observed_at_unix_ms: POLICY_NOW_UNIX_MS + 50,
            detection_budget: None,
        },
    );
    for signal in &signals {
        host.record_policy_planning_signal(signal.clone())
            .expect("planning signal");
    }
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_some(),
        "informational planning facts must not pause or complete execution"
    );

    let failure_input = PolicyExecutionInput::Failed {
        error_code: "transient.capture".to_owned(),
        class: PolicyFailureClass::Recoverable,
    };
    let outcome = host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            POLICY_NOW_UNIX_MS + 100,
            &failure_input,
        )
        .expect("policy failure outcome");
    let PolicyExecutionOutcome::Failed { failure } = &outcome.outcome else {
        panic!("expected classified failure")
    };
    assert_eq!(failure.consecutive_same_error, 1);
    assert_eq!(failure.escalation_streak, 1);
    assert!(!failure.performance_tax_exempt);
    assert_eq!(
        failure.perf_context.health,
        PerformanceMonitorHealth::Unavailable
    );
    assert_eq!(
        failure.perf_context.window_end_unix_ms,
        POLICY_NOW_UNIX_MS + 100
    );
    assert_eq!(
        failure.perf_context.window_start_unix_ms,
        POLICY_NOW_UNIX_MS + 100 - 30_000
    );
    assert_eq!(failure.effective_class, PolicyFailureClass::Recoverable);
    assert_eq!(
        failure.disposition,
        PolicyFailureDisposition::RetryScheduled
    );
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("catalog pin")
            .is_none()
    );

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyPlanningSignalObserved)
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        match &event.payload {
            ProjectionPayload::Full(payload) => matches!(
                payload.as_ref(),
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.admission() == Some(&budget_record)
            ),
            _ => false,
        }
    }));
    drop(client);
    host.close().expect("close host");

    let reopened = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::new(FakeState::default()));
    let replay = reopened
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            POLICY_NOW_UNIX_MS + 100,
            &failure_input,
        )
        .expect("replay recovered policy outcome");
    assert_eq!(replay, outcome);
    for signal in signals {
        reopened
            .record_policy_planning_signal(signal)
            .expect("replay recovered planning signal");
    }
    let mut client = TestClient::connect(&reopened);
    let recovered = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_type == EventType::PolicyPlanningSignalObserved)
            .count(),
        3
    );
    assert_eq!(
        recovered
            .iter()
            .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
            .count(),
        1
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn performance_stutter_is_ledger_visible_and_enriches_policy_failure() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_performance_monitor(PerformanceMonitorConfig::default()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    host.record_pipeline_performance(
        PipelinePerformanceSignal::new(POLICY_INSTANCE_ALIAS, POLICY_NOW_UNIX_MS + 90, 1_500)
            .expect("pipeline signal")
            .with_capture_latency(900)
            .expect("capture latency"),
    )
    .expect("record pipeline performance");

    let outcome = host
        .record_policy_dispatch_outcome(
            &intent.decision_id,
            POLICY_NOW_UNIX_MS + 100,
            &PolicyExecutionInput::Failed {
                error_code: "transient.capture".to_owned(),
                class: PolicyFailureClass::Recoverable,
            },
        )
        .expect("policy failure outcome");
    let PolicyExecutionOutcome::Failed { failure } = outcome.outcome else {
        panic!("expected failure")
    };
    assert_eq!(failure.perf_context.max_frame_gap_ms, Some(1_500));
    assert_eq!(failure.perf_context.max_capture_latency_ms, Some(900));
    assert!(!failure.perf_context.related_event_ids.is_empty());

    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PerformanceStutterDetected)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");
}

#[test]
fn release_sets_switch_atomically_rollback_and_recover_without_duplicate_events() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let (first, first_sources) = release_set(root.path(), "1.0.0", 'a');
    let (second, second_sources) = release_set(root.path(), "2.0.0", 'b');
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");

    assert_eq!(
        host.stage_release_set(first.clone(), &first_sources)
            .expect("stage first"),
        first
    );
    host.stage_release_set(second.clone(), &second_sources)
        .expect("stage second");
    assert_eq!(
        host.activate_release_set(first.release_id())
            .expect("activate first"),
        first
    );
    assert_eq!(
        host.activate_release_set(second.release_id())
            .expect("activate second"),
        second
    );
    assert_eq!(
        host.rollback_release_set(first.release_id())
            .expect("rollback first"),
        first
    );
    assert_eq!(
        host.active_release_set().expect("active release"),
        Some(first.clone())
    );
    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseStaged)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseActivated)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseRolledBack)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), b"rotated-fingerprint-salt"),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened.active_release_set().expect("recovered release"),
        Some(first)
    );
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::ReleaseStaged)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::ReleaseActivated | EventType::ReleaseRolledBack
                )
            })
            .count(),
        3
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn committed_release_without_ledger_outcome_is_reconciled_on_restart() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let (release, sources) = release_set(root.path(), "1.0.0", 'c');
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.stage_release_set(release.clone(), &sources)
        .expect("stage release");
    host.close().expect("close host");

    let state =
        RuntimeStateStore::open(root.path(), b"different-bootstrap-seed").expect("runtime state");
    let preview = state
        .preview_release_transition(ReleaseTransitionKind::Activate, release.release_id())
        .expect("transition preview");
    state
        .commit_release_transition(&preview)
        .expect("commit transition without ledger outcome");
    drop(state);

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened.active_release_set().expect("active release"),
        Some(release)
    );
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ReleaseActivated),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Full(payload) = &events[0].payload else {
        panic!("expected forensic release payload")
    };
    let EventPayload::Release(ReleasePayload::Activated(payload)) = payload.as_ref() else {
        panic!("expected release activation")
    };
    assert_eq!(
        payload.transition().validation_result(),
        StateValidationResult::Recovered
    );
    assert_eq!(
        payload.transition().recovery_action(),
        StateRecoveryAction::ReplayedCommitted
    );
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn legacy_catalog_pointer_migrates_once_into_authoritative_state() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog");
    host.close().expect("close host");

    for name in [
        RUNTIME_STATE_DATABASE_FILE.to_owned(),
        format!("{RUNTIME_STATE_DATABASE_FILE}-wal"),
        format!("{RUNTIME_STATE_DATABASE_FILE}-shm"),
        RUNTIME_STATE_INTEGRITY_KEY_FILE.to_owned(),
    ] {
        let path = root.path().join(name);
        if path.exists() {
            fs::remove_file(path).expect("remove current state file");
        }
    }
    let legacy = root
        .path()
        .join("policy")
        .join("catalogs")
        .join("active.json");
    fs::write(
        &legacy,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "actingcommand.catalog-state.v1",
            "generation": catalog,
        }))
        .expect("legacy pointer"),
    )
    .expect("write legacy pointer");

    let reopened = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "neutral-release",
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime host");
    assert_eq!(
        reopened
            .active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        catalog.catalog_hash()
    );
    assert!(!legacy.exists());
    let mut client = TestClient::connect(&reopened);
    let events = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::StateMigrated),
            ..EventQuery::default()
        },
    );
    assert_eq!(events.len(), 1);
    let ProjectionPayload::Full(payload) = &events[0].payload else {
        panic!("expected forensic state payload")
    };
    let EventPayload::State(StatePayload::Migrated(payload)) = payload.as_ref() else {
        panic!("expected state migration")
    };
    assert_eq!(payload.migration().state_key(), "policy.catalog.active");
    drop(client);
    reopened.close().expect("close reopened host");
}

#[test]
fn agent_dispatcher_sidecar_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_AGENT_SIDECAR_ROOT") else {
        return;
    };
    let mode = std::env::var("ACTINGCOMMAND_AGENT_SIDECAR_MODE").expect("sidecar mode");
    let mut client = TestClient::connect_state_root(Path::new(&root));
    match mode.as_str() {
        "start" => {
            let subscription = actingcommand_contract::RuntimeSubscriptionRequest::new(
                EventQuery {
                    event_type: Some(EventType::AgentWakeRequested),
                    ..EventQuery::default()
                },
                ProjectionProfile::Normal,
                actingcommand_contract::SubscriptionCursor::default(),
                1_000,
                8,
            )
            .expect("wake subscription");
            let request = client.agent_request(RuntimeOperation::SubscribeEvents {
                request: subscription,
            });
            let receipt = client.send(&request);
            let RuntimeResult::EventBatch { batch } =
                receipt.result().expect("wake subscription result")
            else {
                panic!("expected wake event batch")
            };
            let wake_id = batch
                .events()
                .iter()
                .find_map(|event| match &event.payload {
                    ProjectionPayload::Public(payload) => match payload.as_ref() {
                        PublicEventPayload::Agent(payload) => payload.agent_wake_id(),
                        _ => None,
                    },
                    _ => None,
                })
                .expect("projected wake id");
            let request = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
            let receipt = client.send(&request);
            let RuntimeResult::AgentSessionOpened { context } =
                receipt.result().expect("agent session result")
            else {
                panic!("expected agent session context")
            };
            assert_eq!(context.status().state(), AgentAttentionState::Active);
            assert_eq!(context.projection().len(), 2);
            fs::write(
                Path::new(&root).join("agent-session.json"),
                serde_json::to_vec(&context.status().session_id()).expect("session id bytes"),
            )
            .expect("session marker");
        }
        "resume" => {
            let session_id: AgentSessionId = serde_json::from_str(
                &std::env::var("ACTINGCOMMAND_AGENT_SESSION_ID").expect("session id"),
            )
            .expect("typed session id");
            let request = client.agent_request(RuntimeOperation::ResumeAgentSession { session_id });
            let receipt = client.send(&request);
            let RuntimeResult::AgentSessionObserved { context } =
                receipt.result().expect("agent resume result")
            else {
                panic!("expected resumed agent session")
            };
            assert_eq!(context.status().state(), AgentAttentionState::Active);
            let response = AgentSessionResponse::new(
                session_id,
                AgentResponseDisposition::RetryableFailure,
                "fake_sidecar_failed",
                unix_ms_now().expect("wall clock"),
            )
            .expect("agent response");
            let request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
            let receipt = client.send(&request);
            let RuntimeResult::AgentResponseRecorded { status } =
                receipt.result().expect("agent response result")
            else {
                panic!("expected agent response status")
            };
            assert_eq!(status.state(), AgentAttentionState::PausedNeedsHuman);
            fs::write(Path::new(&root).join("agent-resumed"), b"done").expect("resume marker");
        }
        other => panic!("unexpected sidecar mode: {other}"),
    }
}

#[test]
fn detachable_agent_sidecar_recovers_and_escalates_without_device_authority() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    let fake_state = Arc::new(FakeState::default());
    let agent_config = AgentDispatcherConfig::new(1, 60_000, 2).expect("agent config");
    let host = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:timeline-review-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("timeline wake signal");
    let mut observer = TestClient::connect(&host);
    let wakes = projected_events(
        &mut observer,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    assert_eq!(wakes.len(), 1);
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    assert_eq!(payload.wake().kind(), AgentWakeKind::TimelineReached);
    assert_eq!(
        payload.wake().attention_state(),
        AgentAttentionState::PausedNeedsAgent
    );
    let start = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::agent_dispatcher_sidecar_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_AGENT_SIDECAR_ROOT", root.path())
        .env("ACTINGCOMMAND_AGENT_SIDECAR_MODE", "start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run starting sidecar");
    assert!(start.success());
    let session_id: AgentSessionId = serde_json::from_slice(
        &fs::read(root.path().join("agent-session.json")).expect("session marker"),
    )
    .expect("session id");
    drop(observer);
    host.close().expect("close runtime with active agent");

    let recovered = RuntimeHost::start(
        config(&root).with_agent_dispatcher(agent_config),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::clone(&fake_state),
        )),
    )
    .expect("recovered runtime host");
    let resume = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::agent_dispatcher_sidecar_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_AGENT_SIDECAR_ROOT", root.path())
        .env("ACTINGCOMMAND_AGENT_SIDECAR_MODE", "resume")
        .env(
            "ACTINGCOMMAND_AGENT_SESSION_ID",
            serde_json::to_string(&session_id).expect("session id JSON"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run resumed sidecar");
    assert!(resume.success());
    assert!(root.path().join("agent-resumed").is_file());

    let mut observer = TestClient::connect(&recovered);
    for (event_type, expected) in [
        (EventType::AgentWakeRequested, 1),
        (EventType::AgentSessionStarted, 1),
        (EventType::AgentSessionResumed, 1),
        (EventType::AgentSessionEscalated, 1),
    ] {
        assert_eq!(
            projected_events(
                &mut observer,
                EventQuery {
                    event_type: Some(event_type),
                    ..EventQuery::default()
                },
            )
            .len(),
            expected
        );
    }
    let request = observer.agent_request(RuntimeOperation::AgentSessionStatus { session_id });
    let receipt = observer.send(&request);
    let RuntimeResult::AgentSessionObserved { context } =
        receipt.result().expect("agent status result")
    else {
        panic!("expected agent status")
    };
    assert_eq!(
        context.status().state(),
        AgentAttentionState::PausedNeedsHuman
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    let drift = PolicyPlanningSignalEventData {
        signal_id: "signal:drift-review-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    };
    recovered
        .record_policy_planning_signal(drift.clone())
        .expect("drift wake signal");
    recovered
        .record_policy_planning_signal(drift)
        .expect("idempotent drift signal");
    let wakes = projected_events(
        &mut observer,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    assert_eq!(wakes.len(), 2);
    assert!(wakes.iter().any(|event| {
        matches!(
            &event.payload,
            ProjectionPayload::Full(payload)
                if matches!(
                    payload.as_ref(),
                    EventPayload::Agent(AgentPayload::WakeRequested(payload))
                        if payload.wake().kind() == AgentWakeKind::DriftPredicted
                )
        )
    }));
    drop(observer);
    recovered.close().expect("close recovered runtime");
}

#[test]
fn agent_session_start_and_completion_are_idempotent() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:agent-completion-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("timeline wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let wake_id = payload.wake().wake_id();

    let first = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
    let first = client.send(&first);
    let RuntimeResult::AgentSessionOpened { context } =
        first.result().expect("first session result")
    else {
        panic!("expected first session context")
    };
    let session_id = context.status().session_id();
    let replay = client.agent_request(RuntimeOperation::StartAgentSession { wake_id });
    let replay = client.send(&replay);
    let RuntimeResult::AgentSessionOpened { context } = replay.result().expect("replay result")
    else {
        panic!("expected replayed session context")
    };
    assert_eq!(context.status().session_id(), session_id);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionStarted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );

    let response = AgentSessionResponse::new(
        session_id,
        AgentResponseDisposition::Completed,
        "fake_sidecar_completed",
        unix_ms_now().expect("wall clock"),
    )
    .expect("agent response");
    let request = client.agent_request(RuntimeOperation::RecordAgentResponse { response });
    for reconnect in [false, true] {
        if reconnect {
            drop(client);
            client = TestClient::connect(&host);
        }
        let receipt = client.send(&request);
        let RuntimeResult::AgentResponseRecorded { status } =
            receipt.result().expect("completion result")
        else {
            panic!("expected completion status")
        };
        assert_eq!(status.state(), AgentAttentionState::Completed);
    }
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionCompleted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn agent_session_timeout_escalates_to_human() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 20, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:agent-timeout-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("drift wake signal");
    let mut client = TestClient::connect(&host);
    let wakes = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::AgentWakeRequested),
            ..EventQuery::default()
        },
    );
    let ProjectionPayload::Full(payload) = &wakes[0].payload else {
        panic!("expected forensic wake payload")
    };
    let EventPayload::Agent(AgentPayload::WakeRequested(payload)) = payload.as_ref() else {
        panic!("expected agent wake payload")
    };
    let request = client.agent_request(RuntimeOperation::StartAgentSession {
        wake_id: payload.wake().wake_id(),
    });
    let receipt = client.send(&request);
    let RuntimeResult::AgentSessionOpened { context } =
        receipt.result().expect("agent session result")
    else {
        panic!("expected agent session context")
    };
    let session_id = context.status().session_id();

    let mut state = AgentAttentionState::Active;
    for _ in 0..200 {
        thread::sleep(Duration::from_millis(10));
        let request = client.agent_request(RuntimeOperation::AgentSessionStatus { session_id });
        let receipt = client.send(&request);
        let RuntimeResult::AgentSessionObserved { context } =
            receipt.result().expect("agent status result")
        else {
            panic!("expected agent status")
        };
        state = context.status().state();
        if state == AgentAttentionState::PausedNeedsHuman {
            break;
        }
    }
    assert_eq!(state, AgentAttentionState::PausedNeedsHuman);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::AgentSessionEscalated),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn agent_wake_is_reconciled_from_a_committed_planning_signal() {
    let root = TempDir::new().expect("tempdir");
    let runtime_instance_id = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:recover-wake-a".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::DriftPredicted,
        fact_code: "goal.primary.drift_predicted".to_owned(),
        observed_at_unix_ms: unix_ms_now().expect("wall clock"),
        detection_budget: None,
    })
    .expect("planning signal");
    host.close().expect("close host");

    let reopened = RuntimeHost::start(
        config(&root)
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            runtime_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("reopen runtime");
    let mut observer = TestClient::connect(&reopened);
    assert_eq!(
        projected_events(
            &mut observer,
            EventQuery {
                event_type: Some(EventType::AgentWakeRequested),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(observer);
    reopened.close().expect("close reopened runtime");
}

#[test]
fn proposal_a_b_c_pipeline_requires_reports_and_authoritative_approvals() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("base catalog");
    let report = host
        .store_test_report(b"synthetic immutable strategy report")
        .expect("strategy report");
    let mut client = TestClient::connect(&host);

    let proposal_a = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![report.clone()],
        ProposalKind::ParameterInstantiation {
            instantiation: TaskTemplateInstantiation::new(
                "fixture.observe",
                "fixture.observe-copy",
                POLICY_INSTANCE_ALIAS,
                Some(110),
                Some(1_100),
            )
            .expect("template instantiation"),
        },
    )
    .expect("class A proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalEvaluated { preview: preview_a } =
        receipt.result().expect("proposal preview")
    else {
        panic!("expected proposal preview")
    };
    assert_eq!(preview_a.class(), ProposalClass::A);
    assert_eq!(
        preview_a.disposition(),
        ProposalDisposition::ReadyForApproval
    );

    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-plan-a",
        preview_a.approval_target().expect("plan target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-template-a",
        ApprovalTarget::Catalog {
            catalog_hash: base.catalog_hash().to_owned(),
            catalog_version: base.catalog_version(),
        },
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalPromoted {
        promotion: promotion_a,
    } = receipt.result().expect("proposal promotion")
    else {
        panic!("expected proposal promotion")
    };
    assert_eq!(promotion_a.preview().class(), ProposalClass::A);
    assert_eq!(promotion_a.approval_fact_ids().len(), 2);
    let activated = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogActivated),
            ..EventQuery::default()
        },
    );
    let authorization = activated
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Catalog(CatalogPayload::Activated(payload)) => payload.promotion(),
                _ => None,
            },
            _ => None,
        })
        .expect("durable proposal authorization");
    assert_eq!(authorization.proposal_id(), proposal_a.proposal_id());
    assert_eq!(authorization.class(), ProposalClass::A);
    assert_eq!(
        authorization.approval_fact_ids(),
        promotion_a.approval_fact_ids()
    );
    assert_eq!(authorization.report_artifact_ids(), [report.artifact_id]);
    let active_a = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    assert_eq!(active_a.catalog_version(), 2);
    assert_eq!(
        active_a.catalog_hash(),
        preview_a.target_catalog_hash().expect("target hash")
    );
    let cycle = host
        .evaluate_policy_cycle(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::CatalogChanged,
        )
        .expect("evaluate promoted catalog");
    assert!(
        cycle
            .evaluation
            .expect("policy evaluation")
            .decisions
            .iter()
            .any(|decision| decision.task_id == "fixture.observe-copy")
    );
    let activated_before_replay = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::CatalogActivated),
            ..EventQuery::default()
        },
    )
    .len();
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_a),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::CatalogActivated),
                ..EventQuery::default()
            },
        )
        .len(),
        activated_before_replay
    );

    let mut patches = proposal_version_patches(3);
    patches.push(
        CatalogDeclarationPatch::new(
            ProposalDocument::Tasks,
            ProposalPatchOperation::Replace,
            "/tasks/0/priority",
            Some("111".to_owned()),
        )
        .expect("priority patch"),
    );
    let proposal_b = CatalogProposal::new(
        active_a.catalog_hash(),
        active_a.catalog_version(),
        3,
        vec![report.clone()],
        ProposalKind::CatalogDiff { patches },
    )
    .expect("class B proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_b.clone()),
    });
    let receipt = client.send(&request);
    let RuntimeResult::ProposalEvaluated { preview: preview_b } =
        receipt.result().expect("class B preview")
    else {
        panic!("expected class B preview")
    };
    assert_eq!(preview_b.class(), ProposalClass::B);
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_b.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:proposal-plan-b",
        preview_b.approval_target().expect("class B plan target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_b),
    });
    let receipt = client.send(&request);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ProposalPromoted { promotion })
            if promotion.preview().class() == ProposalClass::B
                && promotion.approval_fact_ids() == ["approval:proposal-plan-b"]
    ));
    let active_b = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    assert_eq!(active_b.catalog_version(), 3);

    let proposal_c = CatalogProposal::new(
        active_b.catalog_hash(),
        active_b.catalog_version(),
        4,
        vec![report],
        ProposalKind::LanguageExtension {
            extension_code: "predicate.new-observation".to_owned(),
        },
    )
    .expect("class C proposal");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(proposal_c.clone()),
    });
    let receipt = client.send(&request);
    assert!(matches!(
        receipt.result(),
        Some(RuntimeResult::ProposalEvaluated { preview })
            if preview.class() == ProposalClass::C
                && preview.disposition() == ProposalDisposition::NeedsHumanSpecification
                && preview.approval_target().is_none()
    ));
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal_c),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_version(),
        3
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn proposal_rejects_unverified_reports_and_invalid_packs_without_partial_activation() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("base catalog");
    let report = host
        .store_test_report(b"synthetic immutable strategy report")
        .expect("strategy report");
    let mut client = TestClient::connect(&host);

    let forged = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![unverified_report(&report, &client.ids)],
        ProposalKind::ParameterInstantiation {
            instantiation: TaskTemplateInstantiation::new(
                "fixture.observe",
                "fixture.unverified",
                POLICY_INSTANCE_ALIAS,
                None,
                None,
            )
            .expect("template instantiation"),
        },
    )
    .expect("forged proposal shape");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(forged),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert!(host.fatal_error().expect("runtime health").is_none());

    let mut patches = proposal_version_patches(2);
    patches.push(
        CatalogDeclarationPatch::new(
            ProposalDocument::Tasks,
            ProposalPatchOperation::Replace,
            "/tasks/0/priority",
            Some("\"not-an-integer\"".to_owned()),
        )
        .expect("invalid typed priority patch"),
    );
    let invalid_pack = CatalogProposal::new(
        base.catalog_hash(),
        base.catalog_version(),
        2,
        vec![report],
        ProposalKind::CatalogDiff { patches },
    )
    .expect("invalid compiled proposal shape");
    let request = client.agent_request(RuntimeOperation::CompileProposal {
        proposal: Box::new(invalid_pack),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    assert_eq!(
        fs::read_dir(root.path().join("policy/catalogs/generations"))
            .expect("catalog generations")
            .count(),
        1
    );
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::CatalogActivated),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_report_is_local_deterministic_and_promotes_only_after_approval() {
    let root = TempDir::new().expect("tempdir");
    let fake_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&fake_state),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic pinned strategy evidence")
        .expect("strategy evidence");
    let report = strategy_report(
        &base,
        &evidence,
        verified_artifact_sequence(&host, &evidence),
    );

    let first = host
        .prepare_strategic_report(&report, std::slice::from_ref(&evidence))
        .expect("first strategy preparation");
    let second = host
        .prepare_strategic_report(&report, std::slice::from_ref(&evidence))
        .expect("replayed strategy preparation");
    assert_eq!(first, second);
    assert_eq!(first.report().kind(), ArtifactKind::StrategyReport);
    assert_eq!(first.projection().instances.len(), 2);
    assert!(first.projection().instances.iter().any(|projection| {
        projection.band == StrategicBand::InfeasibleBestEffort
            && projection.planning_disposition
                == actingcommand_policy::PlanningDisposition::ExecutionContinues
    }));
    assert_eq!(first.projection().additions.tasks.len(), 2);
    assert_eq!(first.projection().additions.activity_profiles.len(), 2);
    assert!(
        first
            .projection()
            .additions
            .tasks
            .iter()
            .all(|task| matches!(task.scope, ScopeSelector::Instance { .. }))
    );
    assert!(
        first
            .projection()
            .additions
            .activity_profiles
            .iter()
            .all(|profile| matches!(profile.scope, ScopeSelector::Instance { .. }))
    );
    let proposal = first.proposal().expect("mechanical catalog proposal");
    assert_eq!(proposal.class(), ProposalClass::B);
    assert_eq!(proposal.report_refs(), [first.report().clone()]);
    assert_eq!(
        first
            .preview()
            .expect("strategy proposal preview")
            .proposal_id(),
        proposal.proposal_id()
    );
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    let stored =
        read_projected_verified(root.path(), first.report()).expect("local strategic report bytes");
    assert_eq!(
        serde_json::from_slice::<StrategicReport>(&stored).expect("stored strategic report"),
        report
    );

    let mut client = TestClient::connect(&host);
    let strategy_artifacts = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            ..EventQuery::default()
        },
    )
    .into_iter()
    .flat_map(|event| event.artifacts)
    .filter(|artifact| artifact.kind() == ArtifactKind::StrategyReport)
    .count();
    assert_eq!(strategy_artifacts, 1);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::PolicyExecutionRecorded),
                ..EventQuery::default()
            }
        )
        .is_empty()
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal.clone()),
    });
    assert_eq!(client.send(&request).state(), RuntimeReceiptState::Denied);
    record_target_approval(
        &mut client,
        "approval:strategy-plan",
        first
            .preview()
            .expect("strategy preview")
            .approval_target()
            .expect("strategy approval target"),
    );
    let request = client.agent_request(RuntimeOperation::PromoteProposal {
        proposal: Box::new(proposal.clone()),
    });
    assert_eq!(
        client.send(&request).state(),
        RuntimeReceiptState::Completed
    );
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_version(),
        2
    );
    assert_eq!(fake_state.open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.capture_open_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake_state.input_count.load(Ordering::SeqCst), 0);
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn strategic_report_rejects_unverified_evidence_without_artifact_or_catalog_change() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let base = host
        .activate_policy_catalog(&strategy_policy_sources(1))
        .expect("strategy base catalog");
    let evidence = host
        .store_test_report(b"synthetic pinned strategy evidence")
        .expect("strategy evidence");
    let evidence_sequence = verified_artifact_sequence(&host, &evidence);
    let report = strategy_report(&base, &evidence, evidence_sequence);
    let stale_report = strategy_report(&base, &evidence, evidence_sequence - 1);
    let error = host
        .prepare_strategic_report(&stale_report, std::slice::from_ref(&evidence))
        .expect_err("evidence newer than the report as-of position must fail");
    assert_eq!(error.code(), "strategic_evidence_unverified");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let forged = unverified_report(&evidence, &ids);

    let error = host
        .prepare_strategic_report(&report, &[forged])
        .expect_err("unverified strategy evidence must fail");
    assert_eq!(error.code(), "strategic_evidence_unverified");
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.active_policy_catalog()
            .expect("active catalog")
            .expect("catalog")
            .catalog_hash(),
        base.catalog_hash()
    );
    let mut client = TestClient::connect(&host);
    assert!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::ArtifactVerified),
                ..EventQuery::default()
            }
        )
        .iter()
        .flat_map(|event| event.artifacts.iter())
        .all(|artifact| artifact.kind() != ArtifactKind::StrategyReport)
    );
    drop(client);
    host.close().expect("close runtime");
}

#[test]
fn policy_dispatch_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_POLICY_CRASH_ROOT") else {
        return;
    };
    let instance_bytes = fs::read(Path::new(&root).join("instance.json")).expect("instance bytes");
    let instance_id: InstanceId =
        serde_json::from_slice(&instance_bytes).expect("instance identifier");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"policy-crash-process-salt")
            .with_governance_capability(TEST_GOVERNANCE_CAPABILITY),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("child runtime host");
    let catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("child catalog activation");
    let (_, intent, reason_chain) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert_eq!(intent.catalog_hash, catalog.catalog_hash());
    record_policy_approval(&host, &intent);
    if std::env::var_os("ACTINGCOMMAND_POLICY_CRASH_POINT").is_some() {
        fs::write(
            Path::new(&root).join("dispatch-before-crash.json"),
            serde_json::to_vec(&(intent.clone(), reason_chain.clone()))
                .expect("pending dispatch bytes"),
        )
        .expect("pending dispatch marker");
    }
    let admission = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
        .expect("child policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    fs::write(
        Path::new(&root).join("admitted-before-crash.json"),
        serde_json::to_vec(&(intent, reason_chain)).expect("admitted dispatch bytes"),
    )
    .expect("admitted dispatch marker");
    fs::write(Path::new(&root).join("child-ready"), b"ready").expect("child marker");
    std::process::exit(0);
}

#[test]
fn policy_pending_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_POLICY_PENDING_ROOT") else {
        return;
    };
    let instance_bytes =
        fs::read(Path::new(&root).join("pending-instances.json")).expect("instance bytes");
    let (instance_a, instance_b): (InstanceId, InstanceId) =
        serde_json::from_slice(&instance_bytes).expect("instance identifiers");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"policy-pending-process-salt")
            .with_governance_capability(TEST_GOVERNANCE_CAPABILITY),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_a,
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_b,
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("pending child runtime host");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("pending child catalog activation");
    let cycle = host
        .evaluate_policy_cycle(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("pending child evaluation");
    let evaluation = cycle
        .evaluation
        .as_ref()
        .expect("pending child evaluation data");
    assert_eq!(cycle.pending_dispatch_intents.len(), 2);
    let pairs = cycle
        .pending_dispatch_intents
        .iter()
        .map(|intent| {
            let reason = evaluation
                .reason_chains
                .iter()
                .find(|reason| reason.id == intent.reason_chain_id)
                .expect("pending child reason chain");
            (intent.clone(), reason.clone())
        })
        .collect::<Vec<_>>();
    let admitted = pairs
        .iter()
        .find(|(intent, _)| intent.instance_id == POLICY_INSTANCE_ALIAS)
        .expect("admitted child intent")
        .clone();
    let pending = pairs
        .iter()
        .find(|(intent, _)| intent.instance_id == POLICY_INSTANCE_ALIAS_B)
        .expect("pending child intent")
        .clone();
    record_policy_approval(&host, &admitted.0);
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted.0,
            &admitted.1,
            &policy_context(&host, &admitted.0),
        )
        .expect("pending child admission"),
        PolicyDispatchAdmission::Granted { .. }
    ));
    fs::write(
        Path::new(&root).join("nonempty-pending-before-crash.json"),
        serde_json::to_vec(&(admitted, pending)).expect("pending crash marker bytes"),
    )
    .expect("pending crash marker");
    std::process::exit(86);
}

#[test]
fn orphaned_policy_admission_is_reconciled_after_real_process_kill() {
    for (point, expected_effect, expected_lease_grants) in [
        ("after_policy_intent", EffectDisposition::NotPerformed, 0),
        ("after_lease_grant", EffectDisposition::Indeterminate, 1),
        ("after_budget_commit", EffectDisposition::Indeterminate, 1),
    ] {
        let root = TempDir::new().expect("tempdir");
        let shared_instance_id = instance_id();
        fs::write(
            root.path().join("instance.json"),
            serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
        )
        .expect("instance file");
        let marker = root.path().join("policy-crash-marker");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::policy_dispatch_crash_child_process",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
            .env("ACTINGCOMMAND_POLICY_CRASH_POINT", point)
            .env("ACTINGCOMMAND_POLICY_CRASH_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn crash child");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.is_file() {
            assert!(Instant::now() < deadline, "crash marker timeout at {point}");
            assert!(
                child.try_wait().expect("poll crash child").is_none(),
                "crash child exited before {point}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().expect("kill crash child");
        let status = child.wait().expect("wait crash child");
        assert!(!status.success());

        let (intent, reason_chain): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
            &fs::read(root.path().join("dispatch-before-crash.json"))
                .expect("pending dispatch marker"),
        )
        .expect("pending dispatch JSON");
        let host = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("recovered runtime host");
        assert!(
            host.pinned_policy_catalog(&intent.decision_id)
                .expect("pinned catalog")
                .is_none()
        );
        assert!(matches!(
            host.admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
                .expect("replay reconciled dispatch"),
            PolicyDispatchAdmission::ReplaySuppressed { .. }
        ));

        let mut client = TestClient::connect(&host);
        let events = projected_events(&mut client, EventQuery::default());
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchAdmitted)
                .count(),
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::LeaseGranted)
                .count(),
            expected_lease_grants
        );
        let rejection = events
            .iter()
            .find(|event| event.event_type == EventType::PolicyDispatchRejected)
            .expect("reconciled rejection");
        let ProjectionPayload::Full(payload) = &rejection.payload else {
            panic!("expected forensic rejection payload")
        };
        let EventPayload::Policy(PolicyPayload::DispatchRejected(_)) = payload.as_ref() else {
            panic!("expected policy rejection payload")
        };
        assert_eq!(payload.effect_disposition(), Some(expected_effect));
        drop(client);

        thread::sleep(Duration::from_millis(50));
        let (_, next_intent, next_reasons) =
            evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
        record_policy_approval(&host, &next_intent);
        let admission = host
            .admit_policy_dispatch(
                &next_intent,
                &next_reasons,
                &policy_context(&host, &next_intent),
            )
            .expect("post-recovery admission");
        let PolicyDispatchAdmission::Granted { admission, .. } = admission else {
            panic!("expected post-recovery grant")
        };
        assert_eq!(admission.budget.task_daily_used, 1);
        assert_eq!(admission.budget.activity_window_used, 1);
        host.close().expect("close recovered host");

        let reopened = RuntimeHost::start(
            config(&root),
            Arc::new(FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                shared_instance_id,
                Arc::new(FakeState::default()),
            )),
        )
        .expect("reopen reconciled runtime host");
        assert!(
            reopened
                .pinned_policy_catalog(&intent.decision_id)
                .expect("reopened pinned catalog")
                .is_none()
        );
        let mut client = TestClient::connect(&reopened);
        assert_eq!(
            projected_events(&mut client, EventQuery::default())
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchRejected)
                .count(),
            1
        );
        drop(client);
        reopened.close().expect("close reopened host");
    }
}

#[test]
fn policy_dispatch_survives_real_process_crash_without_second_lease_side_effect() {
    let root = TempDir::new().expect("tempdir");
    let shared_instance_id = instance_id();
    fs::write(
        root.path().join("instance.json"),
        serde_json::to_vec(&shared_instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::policy_dispatch_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_POLICY_CRASH_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run crash child");
    assert!(status.success());
    assert!(root.path().join("child-ready").is_file());

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            shared_instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("recovered runtime host");
    let catalog = host
        .active_policy_catalog()
        .expect("active catalog")
        .expect("catalog");
    let (cycle, _, _) = evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
    assert_eq!(cycle.directive.kind, PolicyRecomputeKind::Full);
    let (intent, reason_chain): (DispatchIntent, DecisionReasonChain) = serde_json::from_slice(
        &fs::read(root.path().join("admitted-before-crash.json"))
            .expect("admitted dispatch marker"),
    )
    .expect("admitted dispatch JSON");
    assert_eq!(intent.catalog_hash, catalog.catalog_hash());
    let replay = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host, &intent))
        .expect("replay after crash");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let mut client = TestClient::connect(&host);
    let events = projected_events(&mut client, EventQuery::default());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::PolicyDispatchIntent)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::LeaseGranted)
            .count(),
        1
    );
    drop(client);
    host.close().expect("close recovered host");
}

#[test]
fn nonempty_pending_policy_work_is_rebuilt_after_real_process_crash() {
    let root = TempDir::new().expect("tempdir");
    let instance_a = instance_id();
    let instance_b = instance_id();
    fs::write(
        root.path().join("pending-instances.json"),
        serde_json::to_vec(&(instance_a, instance_b)).expect("pending instance bytes"),
    )
    .expect("pending instance file");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::policy_pending_crash_child_process",
            "--nocapture",
        ])
        .env("ACTINGCOMMAND_POLICY_PENDING_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run pending crash child");
    assert!(!status.success());

    let (admitted, pending): (
        (DispatchIntent, DecisionReasonChain),
        (DispatchIntent, DecisionReasonChain),
    ) = serde_json::from_slice(
        &fs::read(root.path().join("nonempty-pending-before-crash.json"))
            .expect("nonempty pending marker"),
    )
    .expect("nonempty pending marker JSON");
    assert_ne!(admitted.0.instance_id, pending.0.instance_id);

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_a,
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_b,
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("recovered pending runtime host");
    let cycle = host
        .evaluate_policy_cycle(
            &pending_policy_facts(),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 60_000,
                monotonic_ms: POLICY_NOW_UNIX_MS + 60_000,
            },
            8,
            PolicyTrigger::Recovery,
        )
        .expect("rebuild pending policy work");
    assert!(!cycle.pending_dispatch_intents.is_empty());
    assert!(cycle.pending_dispatch_intents.iter().any(|intent| {
        intent.task_id == pending.0.task_id && intent.instance_id == pending.0.instance_id
    }));
    assert!(matches!(
        host.admit_policy_dispatch(
            &admitted.0,
            &admitted.1,
            &policy_context(&host, &admitted.0),
        )
        .expect("replay admitted work after crash"),
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    let mut client = TestClient::connect(&host);
    assert_eq!(
        projected_events(
            &mut client,
            EventQuery {
                event_type: Some(EventType::LeaseGranted),
                ..EventQuery::default()
            },
        )
        .len(),
        1
    );
    drop(client);
    host.close().expect("close recovered pending runtime");
}
