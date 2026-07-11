// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, InputAction, InstanceId,
    ProjectionProfile,
};
use actingcommand_device::{DeviceResult, InputBackend};
use actingcommand_runtime_host::{
    InputBackendProvider, ResolvedInputInstance, RuntimeHost, RuntimeHostConfig,
};
use actingcommand_scheduler::SchedulerConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

#[derive(Default)]
struct FakeState {
    opens: AtomicUsize,
    inputs: AtomicUsize,
    closes: AtomicUsize,
}

struct FakeBackend {
    state: Arc<FakeState>,
    closed: bool,
}

impl FakeBackend {
    fn input(&self) -> DeviceResult<()> {
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

impl InputBackendProvider for FakeProvider {
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedInputInstance> {
        (instance_alias == "ak.cn")
            .then(|| ResolvedInputInstance::new(self.instance_id, "127.0.0.1:16384"))
    }

    fn open(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        assert_eq!(instance_alias, "ak.cn");
        self.state.opens.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeBackend {
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
    let capability = client.admit_readonly("ak.cn").expect("readonly admission");
    let token = client.acquire_lease("ak.cn").expect("lease");
    assert_eq!(capability.instance_id(), token.instance_id());
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
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn runtime_input_proxy_renews_before_short_lease_expiry() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 80);
    let client = client(&root);
    let mut proxy = RuntimeInputProxy::connect_with_heartbeat(
        client.clone(),
        "ak.cn",
        Duration::from_millis(10),
    )
    .expect("runtime input proxy");

    thread::sleep(Duration::from_millis(220));
    proxy.tap(30, 40).expect("input after renewals");
    proxy.close().expect("close proxy");
    assert_eq!(state.inputs.load(Ordering::Acquire), 1);
    assert_eq!(state.closes.load(Ordering::Acquire), 1);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn long_input_extends_only_its_response_wait() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host(&root, Arc::clone(&state), 1_000);
    let client = client_with_timeout(&root, Duration::from_millis(40));
    let token = client.acquire_lease("ak.cn").expect("lease");

    client
        .input(
            &token,
            InputAction::LongTap {
                x: 10,
                y: 20,
                duration_ms: 80,
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
