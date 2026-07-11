// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::time::unix_ms_now;
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, InputAction, InstanceId,
    LeaseToken, ProjectionProfile, RUNTIME_INFO_FILE, RuntimeErrorCode, RuntimeOperation,
    RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use actingcommand_device::{DeviceError, DeviceResult, InputBackend};
use actingcommand_scheduler::{ConnectionId, SchedulerConfig};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[derive(Default)]
struct FakeState {
    open_count: AtomicUsize,
    input_count: AtomicUsize,
    close_count: AtomicUsize,
    fail_input: AtomicBool,
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl FakeBackend {
    fn input(&self) -> DeviceResult<()> {
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

struct FakeEntry {
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

struct FakeProvider {
    entries: BTreeMap<String, FakeEntry>,
}

impl FakeProvider {
    fn one(alias: &str, instance_id: InstanceId, state: Arc<FakeState>) -> Self {
        Self {
            entries: BTreeMap::from([(alias.to_string(), FakeEntry { instance_id, state })]),
        }
    }
}

impl InputBackendProvider for FakeProvider {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedInputInstance> {
        let entry = self.entries.get(instance_alias)?;
        Some(ResolvedInputInstance::new(
            entry.instance_id,
            "127.0.0.1:16384",
        ))
    }

    fn open(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
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
        RuntimeRequest::new(
            self.ids.mint_request_id().expect("request id"),
            self.ids.mint_correlation_id().expect("correlation id"),
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
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn config(root: &TempDir) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root.path(), b"runtime-host-test-salt")
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 200,
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
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);
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
    assert_eq!(state.close_count.load(Ordering::Acquire), 1);
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
    assert_eq!(state.open_count.load(Ordering::Acquire), 1);

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
fn connection_drop_revokes_lease_and_closes_backend() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let _ = client.acquire("ak.cn");
    drop(client);
    wait_until(Duration::from_secs(2), || {
        state.close_count.load(Ordering::Acquire) == 1
    });

    let mut replacement = TestClient::connect(&host);
    let (_, token) = replacement.acquire("ak.cn");
    let release = replacement.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        replacement.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(replacement);
    host.close().expect("close host");
}

#[test]
fn every_fencing_field_is_checked_before_backend_use() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, "ak.cn", Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let (_, token) = client.acquire("ak.cn");
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
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let forged = LeaseToken::new(
        token.owner_epoch(),
        token.lease_id(),
        token.instance_id(),
        *ids.mint_holder_id().expect("holder id").transport(),
        token.expires_at_monotonic_ms(),
    )
    .expect("forged token");
    let input = client.request(RuntimeOperation::Input {
        token: forged,
        action: InputAction::Tap { x: 10, y: 20 },
    });
    let receipt = client.send(&input);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied);
    assert_eq!(
        receipt.error_projection().expect("denial").code,
        RuntimeErrorCode::HolderMismatch
    );
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
fn expired_lease_is_closed_before_a_new_grant() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let provider = Arc::new(FakeProvider::one(
        "ak.cn",
        instance_id(),
        Arc::clone(&state),
    ));
    let host = RuntimeHost::start(
        config(&root).with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 10,
            takeover_cooldown_ms: 20,
            lease_ttl_ms: 60,
        }),
        provider,
    )
    .expect("runtime host");
    let mut first = TestClient::connect(&host);
    let _ = first.acquire("ak.cn");
    wait_until(Duration::from_secs(2), || {
        state.close_count.load(Ordering::Acquire) == 1
    });
    let mut second = TestClient::connect(&host);
    let (_, token) = second.acquire("ak.cn");
    let release = second.request(RuntimeOperation::ReleaseLease { token });
    second.send(&release);
    drop(first);
    drop(second);
    host.close().expect("close host");
}
