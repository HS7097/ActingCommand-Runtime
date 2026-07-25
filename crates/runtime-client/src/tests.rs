// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget, CaptureSequenceSpec,
    ClientActionKind, ClientActionRecord, EventActor, EventQuery, EventSource, EventType,
    IdentifierIssuer, InputAction, InstanceId, LeasePriority, LeaseQueuePolicy, ProjectionProfile,
    ResourceAuthoringEvent, ResourceAuthoringPhase, RuntimeCaptureBackend, RuntimeDebugEvent,
    RuntimeDebugOperation, RuntimeErrorCode, RuntimeErrorProjection, RuntimeMonitorPolicy,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    RuntimeSubscriptionRequest, SubscriptionCursor,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_scheduler::SchedulerConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const TEST_GOVERNANCE_CAPABILITY: &str = "runtime-client-governance-test-capability";

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
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl FakeBackend {
    fn input(&self) -> DeviceResult<()> {
        if self.state.fail_input.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected input failure"));
        }
        self.state.inputs.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

impl InputBackend for FakeBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.input()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, duration_ms: u64) -> DeviceResult<()> {
        thread::sleep(Duration::from_millis(duration_ms));
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
            self.state.closes.fetch_add(1, Ordering::AcqRel);
        }
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
        assert_eq!(instance_alias, "node.a");
        self.state.opens.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
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
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
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
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
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

#[cfg(test)]
mod run_summary_runtime_e2e_tests {
    use crate::{RuntimeClient, RuntimeClientConfig};
    use actingcommand_contract::{
        ApplicationLifecycleAction, ApprovalDecisionRecord, ApprovalDisposition, ApprovalTarget,
        ContainedTaskRequest, EventActor, EventQuery, EventSource, EventType, IdentifierIssuer,
        InstanceId, ProjectionProfile, RunId,
    };
    use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult, InputBackend};
    use actingcommand_policy::{
        CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources, FactValue,
        HostResourceSnapshot, InstanceSnapshot, ObservedOutcome, PoolValueSnapshot,
    };
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, PolicyAdmissionContext, PolicyCadence, PolicyDispatchAdmission,
        PolicyInputSnapshot, PolicyRunContext, PolicyTrigger, ProcedureBinding, ProcedureManifest,
        ResolvedExecutionInstance, RuntimeClock, RuntimeClockSample, RuntimeHost,
        RuntimeHostConfig, RuntimeHostResult,
    };
    use actingcommand_scheduler::SchedulerConfig;
    use std::collections::BTreeSet;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_CLIENT_SETTLEMENT_ROOT";
    const CHILD_TEST: &str =
        "tests::run_summary_runtime_e2e_tests::actual_no_task_failed_recovery_child";
    const GOVERNANCE_CAPABILITY: &str = "runtime-client-settlement-test-capability";
    const INSTANCE_ALIAS: &str = "fixture-instance-a";
    const NOW_UNIX_MS: u64 = 1_699_963_200_000;

    struct PhysicalProvider {
        instance_id: InstanceId,
    }

    impl ExecutionBackendProvider for PhysicalProvider {
        fn instance_aliases(&self) -> Vec<String> {
            vec![INSTANCE_ALIAS.to_owned()]
        }

        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == INSTANCE_ALIAS)
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384"))
        }

        fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
            Err(DeviceError::fatal(
                "physical provider must be rejected before input opens",
            ))
        }

        fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
            Err(DeviceError::fatal(
                "physical provider must be rejected before capture opens",
            ))
        }

        fn control_application(
            &self,
            _instance_alias: &str,
            _action: ApplicationLifecycleAction,
        ) -> DeviceResult<()> {
            Err(DeviceError::fatal(
                "physical provider must be rejected before application control",
            ))
        }
    }

    struct FixedClock {
        unix_ms: AtomicU64,
        monotonic_ms: AtomicU64,
    }

    impl FixedClock {
        fn new() -> Self {
            Self {
                unix_ms: AtomicU64::new(NOW_UNIX_MS),
                monotonic_ms: AtomicU64::new(NOW_UNIX_MS),
            }
        }
    }

    impl RuntimeClock for FixedClock {
        fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
            Ok(RuntimeClockSample {
                unix_ms: self.unix_ms.fetch_add(10, Ordering::AcqRel),
                monotonic_ms: self.monotonic_ms.fetch_add(10, Ordering::AcqRel),
            })
        }
    }

    struct CrashAfterReleaseClock {
        clock: FixedClock,
        marker: PathBuf,
        armed: AtomicBool,
        armed_samples: AtomicU64,
        crash_after_armed_sample: u64,
    }

    impl CrashAfterReleaseClock {
        fn new(marker: PathBuf, crash_after_armed_sample: u64) -> Self {
            Self {
                clock: FixedClock::new(),
                marker,
                armed: AtomicBool::new(false),
                armed_samples: AtomicU64::new(0),
                crash_after_armed_sample,
            }
        }

        fn arm(&self) {
            self.armed.store(true, Ordering::Release);
        }

        fn armed_samples(&self) -> u64 {
            self.armed_samples.load(Ordering::Acquire)
        }
    }

    impl RuntimeClock for CrashAfterReleaseClock {
        fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
            if self.armed.load(Ordering::Acquire) {
                let sample = self.armed_samples.fetch_add(1, Ordering::AcqRel) + 1;
                if sample == self.crash_after_armed_sample {
                    fs::write(&self.marker, b"post-release-before-policy-execution")
                        .expect("write post-release crash marker");
                    std::process::exit(87);
                }
            }
            self.clock.sample()
        }
    }

    fn policy_sources() -> CatalogSources {
        let mut sources = CatalogSources {
            tasks: CatalogDocumentSource::new(
                "memory://runtime-client-settlement/tasks.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/tasks.json")
                    .to_vec(),
            ),
            pools: CatalogDocumentSource::new(
                "memory://runtime-client-settlement/pools.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json")
                    .to_vec(),
            ),
            activity: CatalogDocumentSource::new(
                "memory://runtime-client-settlement/activity.json",
                include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                    .to_vec(),
            ),
            timeline: CatalogDocumentSource::new(
                "memory://runtime-client-settlement/timeline.json",
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
            document["catalog"]["catalog_version"] = serde_json::json!(1);
            source.bytes = serde_json::to_vec(&document).expect("policy fixture bytes");
        }
        sources
    }

    fn policy_inputs() -> PolicyInputSnapshot {
        let facts = EvaluationFacts {
            ledger_position: 1,
            fact_snapshot_id: "snapshot:runtime-client-settlement".to_owned(),
            facts: Vec::new(),
            outcomes: vec![ObservedOutcome {
                task_id: "fixture.observe".to_owned(),
                instance_id: INSTANCE_ALIAS.to_owned(),
                outcome_key: "completed".to_owned(),
                value: FactValue::Boolean(false),
                observed_at_unix_ms: NOW_UNIX_MS,
            }],
            tasks: Vec::new(),
            instances: vec![InstanceSnapshot {
                instance_id: INSTANCE_ALIAS.to_owned(),
                server_id: "fixture-server-a".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                host_id: "fixture-host-a".to_owned(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_owned()],
                preferred_task_ids: Vec::new(),
            }],
        };
        let resources = EvaluationResources {
            pools: vec![PoolValueSnapshot {
                pool_id: "fixture-pool-a".to_owned(),
                value: 10,
                observed_at_unix_ms: NOW_UNIX_MS,
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
        };
        PolicyInputSnapshot::new(facts, resources)
    }

    fn procedure_manifest() -> ProcedureManifest {
        ProcedureManifest::new(
            [(
                "procedure.observe",
                "operation.observe",
                vec!["after_observation".to_owned()],
            )]
            .into_iter()
            .map(|(procedure_ref, operation_id, yield_points)| {
                ProcedureBinding::new(
                    procedure_ref,
                    format!("sha256:{}", "a".repeat(64)),
                    operation_id,
                    yield_points,
                )
                .expect("procedure binding")
            }),
        )
        .expect("procedure manifest")
    }

    fn host_config(root: &Path, clock: Arc<dyn RuntimeClock>) -> RuntimeHostConfig {
        RuntimeHostConfig::new(root, b"runtime-client-settlement-test-salt")
            .with_policy_inputs(policy_inputs())
            .with_procedure_manifest(procedure_manifest())
            .with_governance_capability(GOVERNANCE_CAPABILITY)
            .with_runtime_clock(clock)
            .with_policy_cadence(PolicyCadence {
                debounce_ms: 1,
                cooldown_ms: 1,
                reconciliation_interval_ms: 1,
                clock_jump_threshold_ms: 1_000_000,
            })
            .with_scheduler(SchedulerConfig {
                lease_ttl_ms: 1_000_000,
                ..SchedulerConfig::default()
            })
            .with_io_timeout(Duration::from_millis(500))
    }

    fn start_host(
        root: &Path,
        instance_id: InstanceId,
        clock: Arc<dyn RuntimeClock>,
    ) -> RuntimeHost {
        RuntimeHost::start(
            host_config(root, clock),
            Arc::new(PhysicalProvider { instance_id }),
        )
        .expect("runtime host")
    }

    fn runtime_client(root: &Path) -> RuntimeClient {
        RuntimeClient::connect(
            RuntimeClientConfig::new(root, EventActor::User, EventSource::Ui)
                .with_io_timeout(Duration::from_secs(2)),
        )
        .expect("runtime client")
    }

    fn admit_scheduled_run(
        host: &RuntimeHost,
        root: &Path,
    ) -> (Box<PolicyRunContext>, ContainedTaskRequest) {
        let catalog = host
            .activate_policy_catalog(&policy_sources())
            .expect("activate policy catalog");
        let cycle = host
            .evaluate_policy_cycle(PolicyTrigger::FactsChanged)
            .expect("evaluate policy cycle");
        let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
        let intent = evaluation
            .dispatch_intents
            .first()
            .expect("policy dispatch intent")
            .clone();
        assert_eq!(intent.catalog_hash, catalog.catalog_hash());
        let client = runtime_client(root);
        client
            .authenticate_governance(GOVERNANCE_CAPABILITY)
            .expect("authenticate governance");
        client
            .record_approval_decision(
                ApprovalDecisionRecord::new(
                    "approval:fixture-a",
                    ApprovalDisposition::Approved,
                    ApprovalTarget::Catalog {
                        catalog_hash: intent.catalog_hash.clone(),
                        catalog_version: intent.catalog_version,
                    },
                    "user_confirmed",
                )
                .expect("approval decision"),
            )
            .expect("record approval decision");

        let approved_cycle = host
            .evaluate_policy_cycle(PolicyTrigger::Reconciliation)
            .expect("reevaluate policy cycle after approval");
        let approved_evaluation = approved_cycle
            .evaluation
            .as_ref()
            .expect("approved policy evaluation");
        let intent = approved_evaluation
            .dispatch_intents
            .first()
            .expect("approved policy dispatch intent")
            .clone();
        let reason_chain = approved_evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("approved policy reason chain")
            .clone();

        let admission = host
            .admit_policy_dispatch(
                &intent,
                &reason_chain,
                &PolicyAdmissionContext {
                    fact_ledger_position: intent.input_ledger_position,
                    fact_snapshot_id: intent.fact_snapshot_id.clone(),
                    approval_fact_ids: BTreeSet::from(["approval:fixture-a".to_owned()]),
                    fencing_owner_epoch: host.runtime_info().owner_epoch(),
                    now_unix_ms: intent.prerequisites.evaluated_at_unix_ms,
                },
            )
            .expect("admit policy dispatch");
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("expected scheduled policy admission")
        };
        let request = ContainedTaskRequest::new(
            root.join("physical-provider-not-opened.zip")
                .to_string_lossy()
                .into_owned(),
            context
                .package_digest()
                .strip_prefix("sha256:")
                .expect("policy package digest"),
        )
        .expect("contained task request");
        (context, request)
    }

    #[test]
    fn actual_no_task_failed_recovery_child() {
        let Ok(root) = env::var(CHILD_ROOT_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let instance_id: InstanceId = serde_json::from_slice(
            &fs::read(root.join("instance-id.json")).expect("instance id bytes"),
        )
        .expect("instance id JSON");
        let crash_clock = Arc::new(CrashAfterReleaseClock::new(root.join("crash-marker"), 3));
        let host = start_host(
            &root,
            instance_id,
            Arc::clone(&crash_clock) as Arc<dyn RuntimeClock>,
        );
        let (context, request) = admit_scheduled_run(&host, &root);
        let run_id = context.run_id();
        let task_id = context.task_id();
        let correlation_id = context.correlation_id();
        let lease_id = context.lease_token().lease_id();
        fs::write(
            root.join("scheduled-identities.json"),
            serde_json::to_vec(&(run_id, task_id, correlation_id, lease_id))
                .expect("scheduled identities JSON"),
        )
        .expect("write scheduled identities");

        crash_clock.arm();
        let error = host
            .run_scheduled_contained_task(&context, &request)
            .expect_err("clock must end child after same-chain release");
        panic!(
            "scheduled task returned before crash after {} armed clock samples: {error}",
            crash_clock.armed_samples()
        );
    }

    #[test]
    fn public_summary_recovers_actual_no_task_failed_scheduled_chain() {
        let root = TempDir::new().expect("tempdir");
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance_id = *issuer.mint_instance_id().expect("instance id").transport();
        fs::write(
            root.path().join("instance-id.json"),
            serde_json::to_vec(&instance_id).expect("instance id JSON"),
        )
        .expect("write instance id");

        let mut child = Command::new(env::current_exe().expect("test executable"))
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ROOT_ENV, root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn scheduled failure child");
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll scheduled failure child") {
                break status;
            }
            if Instant::now() >= deadline {
                child
                    .kill()
                    .expect("kill timed out scheduled failure child");
                let _ = child.wait();
                panic!("scheduled failure child timed out");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(87), "post-release crash status");
        assert!(root.path().join("crash-marker").is_file(), "crash marker");
        let (run_id, task_id, correlation_id, lease_id): (
            RunId,
            actingcommand_contract::TaskId,
            actingcommand_contract::CorrelationId,
            actingcommand_contract::LeaseId,
        ) = serde_json::from_slice(
            &fs::read(root.path().join("scheduled-identities.json"))
                .expect("scheduled identities bytes"),
        )
        .expect("scheduled identities JSON");

        let host = start_host(
            root.path(),
            instance_id,
            Arc::new(FixedClock::new()) as Arc<dyn RuntimeClock>,
        );
        let client = runtime_client(root.path());
        let recovery_events = client
            .query_events(
                EventQuery {
                    run_id: Some(run_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .expect("public recovered run events before summary");
        let summary = client
            .summarize_run(run_id)
            .expect("public recovered run summary");
        assert_eq!(summary["status"], "policy_settlement_interrupted");
        assert_eq!(summary["outcome"]["kind"], "policy_settlement_interrupted");
        assert_eq!(summary["outcome"]["result"], "original_cause_unavailable");
        assert_eq!(summary["actual_effect_count"], 0);
        assert_eq!(summary["simulated_effect_count"], 0);
        assert_eq!(summary["effect"], "not_performed");
        assert_ne!(summary["status"], "success");
        assert_ne!(summary["status"], "no_op");
        assert_eq!(
            summary["run_id"],
            serde_json::to_value(run_id).expect("run id summary JSON")
        );
        assert_eq!(
            summary["task_id"],
            serde_json::to_value(task_id).expect("task id summary JSON")
        );
        assert_eq!(
            summary["correlation_id"],
            serde_json::to_value(correlation_id).expect("correlation id summary JSON")
        );
        assert_eq!(
            summary["lease"]["lease_id"],
            serde_json::to_value(lease_id).expect("lease id summary JSON")
        );

        let events = recovery_events;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyExecutionRecorded)
                .count(),
            1,
            "one recovered policy execution"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::PolicyDispatchCompleted)
                .count(),
            1,
            "one recovered policy completion"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == EventType::LeaseReleased)
                .count(),
            1,
            "one same-chain lease release"
        );
        assert!(events.iter().all(|event| {
            !matches!(
                event.event_type,
                EventType::TaskCompleted
                    | EventType::TaskFailed
                    | EventType::TaskEffectIntent
                    | EventType::TaskEffectCompleted
                    | EventType::InputIntent
                    | EventType::InputCommitted
                    | EventType::InputFailed
            )
        }));
        for event in &events {
            assert_eq!(event.links.run_id(), Some(&run_id), "run linkage");
            assert_eq!(event.links.task_id(), Some(&task_id), "task linkage");
            assert_eq!(
                event.links.correlation_id(),
                Some(&correlation_id),
                "correlation linkage"
            );
            if matches!(
                event.event_type,
                EventType::LeaseGranted
                    | EventType::LeaseReleased
                    | EventType::PolicyExecutionRecorded
                    | EventType::PolicyDispatchCompleted
            ) {
                assert_eq!(event.links.lease_id(), Some(&lease_id), "lease linkage");
            }
        }
        host.close().expect("close recovered runtime host");
    }
}
