// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    AgentPayload, AgentSessionId, ApplicationLifecycleAction, EventActor, EventPayload, EventQuery,
    EventSource, EventType, IdentifierIssuer, InstanceId, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind, ProjectionPayload, ProjectionProfile, RUNTIME_INFO_FILE, RuntimeInfo,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
};
use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult, InputBackend};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    AgentDispatcherConfig, ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost,
    RuntimeHostConfig,
};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

const INSTANCE_ALIAS: &str = "node.a";
const PROCESS_TEST_SALT: &str = "actingd-process-test-salt";

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _kill_result = self.0.kill();
            let _wait_result = self.0.wait();
        }
    }
}

#[test]
fn actingd_outlives_disposable_clients_and_accepts_reconnection() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    write_config(&config_path, root.path(), instance_id, false);
    let child = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actingd");
    let mut child = ChildGuard(child);
    wait_for_runtime_info(&mut child.0, root.path());

    let first = connect(root.path());
    let owner_epoch = first.health().expect("first client health");
    drop(first);

    let second = connect(root.path());
    assert_eq!(second.health().expect("second client health"), owner_epoch);
    assert!(child.0.try_wait().expect("process state").is_none());
    drop(second);

    let agent = connect_agent(root.path());
    let wake_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_agent_wake_id()
        .expect("wake id")
        .transport();
    let error = agent
        .start_agent_session(wake_id)
        .expect_err("dispatcher must default to disabled");
    assert_eq!(
        error.projection().map(|projection| projection.code),
        Some(actingcommand_contract::RuntimeErrorCode::InvalidRequest)
    );
    drop(agent);

    child.0.kill().expect("kill actingd");
    assert!(!child.0.wait().expect("wait actingd").success());
}

#[test]
fn actingd_dispatcher_recovers_fake_backend_wake_and_replays_resume() {
    let root = TempDir::new().expect("tempdir");
    let config_path = root.path().join("actingd.json");
    let instance_id = instance_id();
    // The fake provider creates durable wake state; the production daemon must recover it without
    // opening any device backend and must preserve the resume receipt across process replacement.
    seed_agent_wake(root.path(), instance_id);
    write_config(&config_path, root.path(), instance_id, true);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    let info = wait_for_runtime_info(&mut child.0, root.path());
    let client = connect_agent(root.path());
    let wakes = client
        .query_events(
            EventQuery {
                event_type: Some(EventType::AgentWakeRequested),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("query wake events");
    let wake_id = wakes
        .iter()
        .find_map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Agent(AgentPayload::WakeRequested(payload)) => {
                    Some(payload.wake().wake_id())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("agent wake id");
    let session = client
        .start_agent_session(wake_id)
        .expect("start agent session");
    let session_id = session.status().session_id();
    drop(client);

    let resume = agent_resume_request(session_id);
    let first = raw_exchange(&info, &resume);
    assert_eq!(first.state(), RuntimeReceiptState::Completed);
    let RuntimeResult::AgentSessionObserved { context } = first.result().expect("resume result")
    else {
        panic!("expected resumed session")
    };
    assert_eq!(context.status().session_id(), session_id);
    assert_eq!(raw_exchange(&info, &resume), first);
    assert_eq!(resumed_event_count(root.path()), 1);

    child.0.kill().expect("kill first actingd");
    child.0.wait().expect("wait first actingd");
    drop(child);

    let child = start_actingd(&config_path);
    let mut child = ChildGuard(child);
    let restarted_info = wait_for_runtime_info(&mut child.0, root.path());
    assert_ne!(restarted_info.pid(), info.pid());
    assert_eq!(raw_exchange(&restarted_info, &resume), first);
    assert_eq!(resumed_event_count(root.path()), 1);

    child.0.kill().expect("kill restarted actingd");
    child.0.wait().expect("wait restarted actingd");
}

#[test]
fn invalid_startup_returns_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", "missing-actingd-config.json"])
        .output()
        .expect("run actingd");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("FATAL actingd"));
}

fn connect(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Cli, EventSource::Cli)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect runtime")
}

fn connect_agent(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, EventActor::Agent, EventSource::Adapter)
            .with_io_timeout(Duration::from_millis(500)),
    )
    .expect("connect agent runtime")
}

fn write_config(path: &Path, state_root: &Path, instance_id: InstanceId, dispatcher_enabled: bool) {
    let mut value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": PROCESS_TEST_SALT,
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id,
            "application_id": "neutral.application",
            "adb_path": "adb",
            "touch_backend": "maatouch",
            "capture_backend": "adb",
            "push_touch_tool": false
        }]
    });
    if dispatcher_enabled {
        value["agent_dispatcher"] = json!({
            "max_attempts": 2,
            "max_session_ms": 60_000,
            "max_projection_events": 8
        });
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("config json"),
    )
    .expect("write config");
}

fn wait_for_runtime_info(child: &mut Child, state_root: &Path) -> RuntimeInfo {
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(state_root.join(RUNTIME_INFO_FILE))
            && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&bytes)
            && info.pid() == child.id()
        {
            return info;
        }
        if let Some(status) = child.try_wait().expect("process state") {
            panic!("actingd exited before ready with {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "actingd readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn start_actingd(config_path: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args(["--config", config_path.to_str().expect("config path")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actingd")
}

fn seed_agent_wake(state_root: &Path, instance_id: InstanceId) {
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(state_root, PROCESS_TEST_SALT.as_bytes()).with_agent_dispatcher(
            AgentDispatcherConfig::new(2, 60_000, 8).expect("agent dispatcher config"),
        ),
        Arc::new(FakeProvider { instance_id }),
    )
    .expect("seed runtime host");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:actingd-process-dispatcher".to_owned(),
        instance_id: INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        fact_code: "timeline.review.due".to_owned(),
        observed_at_unix_ms: unix_ms_now(),
        detection_budget: None,
    })
    .expect("record planning signal");
    host.close().expect("close seed runtime host");
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn agent_resume_request(session_id: AgentSessionId) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Agent,
        EventSource::Adapter,
        unix_ms_now(),
        RuntimeOperation::ResumeAgentSession { session_id },
    )
    .expect("resume request")
}

fn resumed_event_count(state_root: &Path) -> usize {
    connect_agent(state_root)
        .query_events(
            EventQuery {
                event_type: Some(EventType::AgentSessionResumed),
                ..EventQuery::default()
            },
            ProjectionProfile::Normal,
        )
        .expect("query resumed events")
        .len()
}

fn raw_exchange(info: &RuntimeInfo, request: &RuntimeRequest) -> RuntimeReceipt {
    let mut stream = TcpStream::connect(info.socket_addr().expect("runtime socket"))
        .expect("connect raw runtime client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set write timeout");
    let body = serde_json::to_vec(request).expect("serialize runtime request");
    assert!(!body.is_empty() && body.len() <= 1024 * 1024);
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .expect("write request header");
    stream.write_all(&body).expect("write request body");
    stream.flush().expect("flush request");
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).expect("read receipt header");
    let length = u32::from_be_bytes(header) as usize;
    assert!((1..=1024 * 1024).contains(&length));
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).expect("read receipt body");
    let receipt = serde_json::from_slice::<RuntimeReceipt>(&body).expect("decode runtime receipt");
    receipt.validate().expect("validate runtime receipt");
    receipt
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis()
        .try_into()
        .expect("millisecond timestamp")
}

struct FakeProvider {
    instance_id: InstanceId,
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec![INSTANCE_ALIAS.to_owned()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == INSTANCE_ALIAS)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "127.0.0.1:16384"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        Err(DeviceError::fatal("fake input backend must not be opened"))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Err(DeviceError::fatal(
            "fake capture backend must not be opened",
        ))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "fake application backend must not be opened",
        ))
    }
}
