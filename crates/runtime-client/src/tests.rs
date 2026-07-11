// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, InputAction, InstanceId,
    LeasePriority, LeaseQueuePolicy, ProjectionProfile, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
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
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384"))
    }

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        self.state.opens.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeBackend {
            state: Arc::clone(&self.state),
            closed: false,
        }))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        self.state.capture_opens.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
            closed: false,
        }))
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

fn client_with_timeout(root: &TempDir, io_timeout: Duration) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(root.path(), EventActor::Cli, EventSource::Cli)
            .with_io_timeout(io_timeout),
    )
    .expect("runtime client")
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
    let token = client.acquire_lease("ak.cn").expect("lease");
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
    assert_eq!(state.opens.load(Ordering::Acquire), 1);
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
}

#[test]
fn typed_client_queues_polls_and_cancels_connection_bound_leases() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let first = client(&root);
    let second = client(&root);
    let first_token = first.acquire_lease("ak.cn").expect("first lease");
    let queued = second
        .queue_lease(
            "ak.cn",
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

    let third = first.acquire_lease("ak.cn").expect("third lease");
    let queued = second
        .queue_lease(
            "ak.cn",
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
        .observe_readonly("ak.cn")
        .expect("readonly observation");

    assert_eq!(state.capture_opens.load(Ordering::Acquire), 1);
    assert_eq!(state.captures.load(Ordering::Acquire), 1);
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ReadonlyObservationCompleted { observation })
            if observation.width() == 2
                && observation.height() == 1
                && observation.verdict() == actingcommand_contract::RecognitionVerdict::FrameDecoded
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
fn capture_failure_is_reported_to_runtime_and_never_returns_fake_success() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.fail_capture.store(true, Ordering::Release);
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client(&root);

    let error = client
        .observe_readonly("ak.cn")
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
        .observe_readonly("ak.cn")
        .expect_err("first capture must fail");
    state.fail_capture.store(false, Ordering::Release);

    let second = client
        .observe_readonly("ak.cn")
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
        .observe_readonly("ak.cn")
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

    let output = client.safe_reset("ak.cn").expect("safe reset");

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
        .safe_reset("ak.cn")
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
        "ak.cn",
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
        "ak.cn",
        Duration::from_millis(20),
    )
    .expect("runtime input proxy");

    drop(proxy);
    assert_eq!(state.closes.load(Ordering::Acquire), 0);
    let replacement = client.acquire_lease("ak.cn").expect("replacement lease");
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
    let token = client.acquire_lease("ak.cn").expect("lease");

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
