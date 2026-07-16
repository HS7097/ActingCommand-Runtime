// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::MONITOR_FILE_NAME;
use crate::time::unix_ms_now;
use actingcommand_artifact_store::read_projected_verified;
use actingcommand_contract::{
    ApplicationLifecycleAction, CaptureSequenceSpec, ContainedTaskRequest, EffectDisposition,
    EventActor, EventPayload, EventQuery, EventSource, EventType, FactContent, FactRecord,
    FactScope, FactValue as ContractFactValue, IdentifierIssuer, InputAction, InstanceFactContext,
    InstanceId, IssuedCorrelationId, LeasePriority, LeaseQueuePolicy, LeaseQueueStatus, LeaseToken,
    MonitorDiagnosis, MonitorDisposition, MonitorObservation, MonitorPayload,
    MonitorRecoveryCoordinationReason, MonitorRecoveryKind, OriginModule, PerformanceControlLevel,
    PerformanceMonitorHealth, PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition,
    PolicyPayload, PolicyPlanningSignalEventData, PolicyPlanningSignalKind, ProjectionPayload,
    ProjectionProfile, PublicEventPayload, RUNTIME_INFO_FILE, ResourceAuthoringEvent,
    ResourceAuthoringPhase, RuntimeCaptureBackend, RuntimeErrorCode, RuntimeMonitorPolicy,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TaskOutcome, TaskPayload, TaskSemanticFact,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, DecisionReasonChain, DispatchIntent, EvaluationFacts,
    EvaluationResources, EvaluationTime, FactValue, HostResourceSnapshot, InstanceSnapshot,
    ObservedOutcome, PoolValueSnapshot,
};
use actingcommand_scheduler::{ConnectionId, SchedulerConfig};
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
        let stream =
            TcpStream::connect(host.runtime_info().socket_addr().expect("runtime address"))
                .expect("connect runtime");
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
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

fn host_with_state(root: &TempDir, alias: &str, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(alias, instance_id(), state)),
    )
    .expect("runtime host")
}

const POLICY_INSTANCE_ALIAS: &str = "fixture-instance-a";
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

fn evaluated_policy_dispatch(
    host: &RuntimeHost,
    trigger: PolicyTrigger,
) -> (PolicyCycle, DispatchIntent, DecisionReasonChain) {
    let cycle = host
        .evaluate_policy_cycle(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
            },
            7,
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

fn policy_context(host: &RuntimeHost) -> PolicyAdmissionContext {
    PolicyAdmissionContext {
        fact_ledger_position: 1,
        fact_snapshot_id: "snapshot:fixture-a".to_owned(),
        approval_fact_ids: BTreeSet::from(["approval:fixture-a".to_owned()]),
        fencing_owner_epoch: host.runtime_info().owner_epoch(),
        now_unix_ms: POLICY_NOW_UNIX_MS,
    }
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
    let ak_state = Arc::new(FakeState::default());
    let ba_state = Arc::new(FakeState::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::from_entries([
            ("ba.jp".to_string(), instance_id(), Arc::clone(&ba_state)),
            ("ak.cn".to_string(), instance_id(), Arc::clone(&ak_state)),
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
    assert_eq!(status.instances()[0].instance_alias(), "ak.cn");
    assert_eq!(status.instances()[1].instance_alias(), "ba.jp");
    assert!(
        status
            .instances()
            .iter()
            .all(|instance| !instance.lease_active())
    );
    assert_eq!(ak_state.open_count.load(Ordering::Acquire), 0);
    assert_eq!(ba_state.open_count.load(Ordering::Acquire), 0);

    let acquire = owner.request(RuntimeOperation::acquire_lease(
        "ak.cn",
        owner.ids.mint_holder_id().expect("owner holder"),
    ));
    let acquire = owner.send(&acquire);
    assert!(matches!(
        acquire.result(),
        Some(RuntimeResult::LeaseGranted { .. })
    ));
    let mut waiter = TestClient::connect(&host);
    let queued = waiter.request(RuntimeOperation::queue_lease(
        "ak.cn",
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
    let ak = &status.instances()[0];
    assert!(ak.lease_active());
    assert_eq!(ak.queued_request_count(), 1);
    assert!(!ak.takeover_cooldown_active());
    assert_eq!(ak_state.open_count.load(Ordering::Acquire), 0);

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
                    "ak.cn".to_string(),
                    instance_id(),
                    Arc::new(FakeState::default()),
                ),
                (
                    "hidden.jp".to_string(),
                    instance_id(),
                    Arc::clone(&hidden_state),
                ),
            ])
            .with_inventory(["ak.cn".to_string()]),
        ),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let hidden = client.request(RuntimeOperation::acquire_lease(
        "hidden.jp",
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
                "ak.cn".to_string(),
                duplicate_id,
                Arc::new(FakeState::default()),
            ),
            (
                "ba.jp".to_string(),
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
            "ak.cn",
            configured_id,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let policy = RuntimeMonitorPolicy::new(1_000, "home", false).expect("policy");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
        Arc::new(FakeProvider::one("ak.cn", configured_id, state)),
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
        instance_alias: "ak.cn".to_string(),
    });
    assert!(matches!(
        client.send(&clear).result(),
        Some(RuntimeResult::MonitorCleared { status }) if status.policy().is_none()
    ));
    let cleared_length = fs::metadata(&journal).expect("monitor metadata").len();
    let repeated_clear = client.request(RuntimeOperation::ClearMonitor {
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("ak.cn");
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
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
        Arc::new(FakeProvider::one("ak.cn", instance_id, Arc::clone(&state))),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
        instance_alias: "ak.cn".to_string(),
    });
    assert_eq!(client.send(&clear).state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close host");
    fs::remove_file(root.path().join(object_key)).expect("remove monitor evidence");

    let restarted = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one("ak.cn", instance_id, state)),
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
    let host = host_with_state(&root, "ak.cn", state);
    let mut client = TestClient::connect(&host);
    let configure = client.request(RuntimeOperation::ConfigureMonitor {
        instance_alias: "ak.cn".to_string(),
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
            "ak.cn",
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let first = TestClient::connect(&host);
    let second = TestClient::connect(&host);
    let start = Arc::new(Barrier::new(3));
    let completed = Arc::new(Barrier::new(3));
    let first = concurrent_acquire(first, "ak.cn", Arc::clone(&start), Arc::clone(&completed));
    let second = concurrent_acquire(second, "ak.cn", Arc::clone(&start), Arc::clone(&completed));

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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("ak.cn");
    let (queued_request, status) = second.queue("ak.cn", LeasePriority::Normal, 2_000);
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("ak.cn");
    let holder = second.ids.mint_holder_id().expect("holder id");
    let queued_request = second.request(RuntimeOperation::queue_lease(
        "ak.cn",
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, old_token) = first.acquire("ak.cn");
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

    let (queued_request, status) = second.queue("ak.cn", LeasePriority::High, 2_000);
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
    let host = host_with_state(&root, "ak.cn", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let mut intruder = TestClient::connect(&host);
    let (_, token) = first.acquire("ak.cn");
    let (queued_request, status) = second.queue("ak.cn", LeasePriority::Normal, 2_000);

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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let _ = first.acquire("ak.cn");
    let (queued_request, status) = second.queue("ak.cn", LeasePriority::Normal, 2_000);
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
        "ak.cn",
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
    let (_, expired_token) = first.acquire("ak.cn");
    let (queued_request, status) = second.queue("ak.cn", LeasePriority::Normal, 1_000);

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
    let host = host_with_state(&root, "ak.cn", state);
    let mut first = TestClient::connect(&host);
    let mut second = TestClient::connect(&host);
    let (_, token) = first.acquire("ak.cn");
    let (queued_request, status) = second.queue("ak.cn", LeasePriority::Normal, 50);
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
    let ak_state = Arc::new(FakeState::default());
    let ba_state = Arc::new(FakeState::default());
    let provider = FakeProvider::from_entries([
        ("ak.cn".to_string(), instance_id(), Arc::clone(&ak_state)),
        ("ba.jp".to_string(), instance_id(), Arc::clone(&ba_state)),
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
    let first = run(first, "ak.cn", Arc::clone(&start));
    let second = run(second, "ba.jp", Arc::clone(&start));

    start.wait();
    first.join().expect("first instance client");
    second.join().expect("second instance client");
    for state in [&ak_state, &ba_state] {
        assert_eq!(state.open_count.load(Ordering::Acquire), 1);
        assert_eq!(state.input_count.load(Ordering::Acquire), 1);
        assert_eq!(state.close_count.load(Ordering::Acquire), 0);
    }
    host.close().expect("close host");
    for state in [&ak_state, &ba_state] {
        assert_eq!(state.close_count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn readonly_observation_uses_one_correlation_and_typed_durable_events() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let capture = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::CaptureSequence {
            instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let valid = client.request(RuntimeOperation::CaptureSequence {
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    fs::write(root.path().join("artifacts"), b"blocks artifact directory")
        .expect("block artifact directory");
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let observe = client.request_with_correlation(
        correlation,
        RuntimeOperation::ObserveReadonly {
            instance_alias: "ak.cn".to_string(),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::safe_reset("ak.cn", client.ids.mint_holder_id().expect("holder")),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::safe_reset("ak.cn", ids.mint_holder_id().expect("holder")),
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
        RuntimeOperation::safe_reset("ak.cn", ids.mint_holder_id().expect("holder")),
    );
    let connection = ConnectionId::new(88).expect("connection");
    let first = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "ak.cn",
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
            "ak.cn",
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let health = client.request(RuntimeOperation::Health);
    let health = client.send_result(&health);
    assert!(
        health.is_ok(),
        "health failed: {health:?}; fatal={:?}",
        host.fatal_error()
    );
    let acquire_request = client.request(RuntimeOperation::acquire_lease(
        "ak.cn",
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
    let host = host_with_state(&root, "ak.cn", state);
    let mut client = TestClient::connect(&host);
    let correlation_id = client.ids.mint_correlation_id().expect("correlation id");
    let correlation_transport = *correlation_id.transport();
    let acquire = client.request_with_correlation(
        correlation_id,
        RuntimeOperation::acquire_lease("ak.cn", client.ids.mint_holder_id().expect("holder id")),
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
    let host = host_with_state(&root, "ak.cn", state);
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
    let host = host_with_state(&root, "ak.cn", state);
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
fn acquire_idempotency_recovers_its_durable_terminal_without_a_connection_cache() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::acquire_lease("ak.cn", ids.mint_holder_id().expect("holder id")),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let connection_id = ConnectionId::new(99).expect("connection id");
    let acquire = runtime_request(
        &ids,
        RuntimeOperation::acquire_lease("ak.cn", ids.mint_holder_id().expect("holder id")),
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
    let first = host_with_state(&root, "ak.cn", Arc::clone(&state));
    assert!(root.path().join(RUNTIME_INFO_FILE).is_file());
    let first_epoch = first.runtime_info().owner_epoch();
    let error = match RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            "ak.cn",
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

    let second = host_with_state(&root, "ak.cn", state);
    assert_ne!(second.runtime_info().owner_epoch(), first_epoch);
    second.close().expect("close second host");
}

#[test]
fn owner_journal_recovers_only_an_incomplete_final_record() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "ak.cn", Arc::clone(&state))
        .close()
        .expect("close initial host");
    let owner_path = root.path().join(crate::owner::OWNER_FILE_NAME);
    OpenOptions::new()
        .append(true)
        .open(&owner_path)
        .expect("open owner journal")
        .write_all(br#"{"incomplete"#)
        .expect("append incomplete tail");

    let recovered = host_with_state(&root, "ak.cn", state);
    recovered.close().expect("close recovered host");
    let content = std::fs::read(&owner_path).expect("read owner journal");
    assert!(content.ends_with(b"\n"));
    assert!(!content.windows(10).any(|window| window == b"incomplete"));
}

#[test]
fn complete_owner_journal_corruption_is_fatal() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    host_with_state(&root, "ak.cn", Arc::clone(&state))
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
        Arc::new(FakeProvider::one("ak.cn", instance_id(), state)),
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let _ = client.acquire("ak.cn");
    drop(client);
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);

    let mut replacement = TestClient::connect(&host);
    let (_, token) = replacement.acquire("ak.cn");
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("ak.cn");
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
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("ak.cn");
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
        "ak.cn",
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
    let _ = first.acquire("ak.cn");
    thread::sleep(Duration::from_millis(1_100));
    assert_eq!(state.open_count.load(Ordering::Acquire), 0);
    let mut second = TestClient::connect(&host);
    let (_, token) = second.acquire("ak.cn");
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

    let cooldown = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 100,
            },
            7,
            PolicyTrigger::ResourcesChanged,
        )
        .expect("cooldown policy cycle");
    assert_eq!(cooldown.directive.kind, PolicyRecomputeKind::Deferred);
    assert_eq!(cooldown.directive.reason, PolicyRecomputeReason::Cooldown);
    assert!(cooldown.evaluation.is_none());

    let incremental = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 1_100,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("incremental policy cycle");
    assert_eq!(incremental.directive.kind, PolicyRecomputeKind::Incremental);
    assert_eq!(incremental.directive.reason, PolicyRecomputeReason::Event);

    let clock_jump = host
        .evaluate_policy_cycle(
            &facts,
            &resources,
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS + 7_000,
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
fn policy_host_revalidates_admission_pins_versions_and_replays_without_side_effects() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, Arc::clone(&state));
    let first_catalog = host
        .activate_policy_catalog(&policy_sources(1))
        .expect("activate first catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);

    let mut missing_approval = policy_context(&host);
    missing_approval.approval_fact_ids.clear();
    let mut missing_approval_intent = intent.clone();
    missing_approval_intent.decision_id = "decision:missing-approval".to_owned();
    missing_approval_intent.reason_chain_id = "reason:missing-approval".to_owned();
    let mut missing_approval_reasons = reasons.clone();
    missing_approval_reasons.id = "reason:missing-approval".to_owned();
    missing_approval_reasons.decision_id = "decision:missing-approval".to_owned();
    let error = host
        .admit_policy_dispatch(
            &missing_approval_intent,
            &missing_approval_reasons,
            &missing_approval,
        )
        .expect_err("missing approval must reject");
    assert_eq!(error.code(), "policy_approval_fact_missing");

    let mut tampered_intent = intent.clone();
    tampered_intent.decision_id = "decision:tampered".to_owned();
    tampered_intent.reason_chain_id = "reason:tampered".to_owned();
    tampered_intent.approval_refs.clear();
    let mut tampered_reasons = reasons.clone();
    tampered_reasons.id = "reason:tampered".to_owned();
    tampered_reasons.decision_id = "decision:tampered".to_owned();
    let error = host
        .admit_policy_dispatch(&tampered_intent, &tampered_reasons, &policy_context(&host))
        .expect_err("catalog approval requirements cannot be stripped");
    assert_eq!(error.code(), "policy_intent_catalog_mismatch");

    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host))
        .expect("policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    assert_eq!(
        host.pinned_policy_catalog(&intent.decision_id)
            .expect("pinned catalog")
            .expect("catalog pin")
            .catalog_hash(),
        first_catalog.catalog_hash()
    );

    let mut client = TestClient::connect(&host);
    let before = projected_events(&mut client, EventQuery::default());
    let mut stale_context = policy_context(&host);
    stale_context.now_unix_ms = POLICY_NOW_UNIX_MS + 60_000;
    let replay = host
        .admit_policy_dispatch(&intent, &reasons, &stale_context)
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
        3
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
        2
    );

    let second_catalog = host
        .activate_policy_catalog(&policy_sources(2))
        .expect("activate second catalog");
    assert_ne!(first_catalog.catalog_hash(), second_catalog.catalog_hash());
    assert_eq!(
        host.pinned_policy_catalog(&intent.decision_id)
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
        .admit_policy_dispatch(&old_new_intent, &old_new_reasons, &policy_context(&host))
        .expect_err("new admission cannot use the old catalog");
    assert_eq!(error.code(), "policy_catalog_mismatch");

    host.complete_policy_dispatch(&intent.decision_id)
        .expect("complete policy dispatch");
    assert!(
        host.pinned_policy_catalog(&intent.decision_id)
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
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&reopened))
        .expect("replay after restart");
    assert!(matches!(
        replay,
        PolicyDispatchAdmission::ReplaySuppressed { .. }
    ));
    reopened.close().expect("close reopened host");
}

#[test]
fn measured_contention_gates_deadline_dispatch_and_records_the_conflict() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, POLICY_INSTANCE_ALIAS, state);
    host.activate_policy_catalog(&policy_sources(1))
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

    let (_, mut intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    intent.prerequisites.urgency_milli = 1_000;
    let error = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host))
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
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host))
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
    host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host))
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
fn policy_dispatch_crash_child_process() {
    let Ok(root) = std::env::var("ACTINGCOMMAND_POLICY_CRASH_ROOT") else {
        return;
    };
    let instance_bytes = fs::read(Path::new(&root).join("instance.json")).expect("instance bytes");
    let instance_id: InstanceId =
        serde_json::from_slice(&instance_bytes).expect("instance identifier");
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(&root, b"policy-crash-process-salt"),
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
    let admission = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host))
        .expect("child policy admission");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));
    fs::write(Path::new(&root).join("child-ready"), b"ready").expect("child marker");
    std::process::exit(0);
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
    let (cycle, intent, reason_chain) = evaluated_policy_dispatch(&host, PolicyTrigger::Recovery);
    assert_eq!(cycle.directive.kind, PolicyRecomputeKind::Full);
    assert!(cycle.pending_dispatch_intents.is_empty());
    assert_eq!(intent.catalog_hash, catalog.catalog_hash());
    let replay = host
        .admit_policy_dispatch(&intent, &reason_chain, &policy_context(&host))
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
