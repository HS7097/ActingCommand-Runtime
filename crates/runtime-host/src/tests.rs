// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::time::unix_ms_now;
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, InputAction, InstanceId,
    IssuedCorrelationId, LeasePriority, LeaseQueuePolicy, LeaseQueueStatus, LeaseToken,
    ProjectionProfile, RUNTIME_INFO_FILE, RuntimeErrorCode, RuntimeOperation, RuntimeReceipt,
    RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_scheduler::{ConnectionId, SchedulerConfig};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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
        self.state.capture_count.fetch_add(1, Ordering::AcqRel);
        if self.state.fail_capture.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected capture failure"));
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
        }
    }
}

impl ExecutionBackendProvider for FakeProvider {
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

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
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
    assert!(matches!(
        completed.result(),
        Some(RuntimeResult::ReadonlyObservationCompleted { observation: actual })
            if actual.width() == 2 && actual.height() == 1
    ));
    assert_eq!(
        event_types_for_correlation(&mut client, correlation_id),
        vec![
            EventType::CliCommand,
            EventType::CommandReceived,
            EventType::CommandValidated,
            EventType::SchedulerAdmitted,
            EventType::CaptureRequested,
            EventType::RecognitionRequested,
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
