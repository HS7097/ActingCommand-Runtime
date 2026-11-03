// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-2 (register §1.270; design `rd-1031/backend-switch/model-backend-switch.md` §2.2, §3,
//! §8, §7 HS-2): a `ConfigureSettings` naming an instance's two backend keys switches its touch
//! and capture backends while the Runtime runs, only on a free instance, and commits the setting
//! only once the new backends opened; otherwise the previous ones come back and the switch is
//! refused, named (`backend_switch_refused`). A start writes the backends in effect into the
//! registry before it prepares any instance.
//!
//! The device is the fake provider's: a switch replaces its registered backends and records the
//! input opens before it; an open fails while its touch backend is one a test names. No device,
//! adb or emulator is touched. The instance ids are minted; the names are neutral. Nothing here
//! sleeps: a claim that waits does so in the Runtime, and a test that must meet the Runtime at a
//! point waits on a channel the fake or a checkpoint answers.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameter, ConfigParameterSource, ConfigSettingChange,
    DeviceBackendSwitchedPayload, FactScalar, RuntimeConfigManifest, RuntimeLifecyclePhase,
    RuntimePayload, SchedulingPauseScope, config_parameters_fact_key,
};
use actingcommand_execution_kernel::{
    DiscoveredInstanceBinding, PendingAdbEndpoint, ResolvedInstanceEndpoint,
};
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use serde_json::{Value, json};

const A: &str = "node.a";
const B: &str = "node.b";
/// How long a test waits for the Runtime to reach a point it signals (never a sleep).
const MEET: Duration = Duration::from_secs(30);

fn id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

fn touch_key(instance: InstanceId) -> String {
    instance_key(&id_text(instance), TOUCH_BACKEND)
}

fn capture_key(instance: InstanceId) -> String {
    instance_key(&id_text(instance), CAPTURE_BACKEND)
}

/// The file layer's two backend rows of `instance` (explicit, as actingd reports them).
fn backend_rows(instance: InstanceId, touch: &str, capture: &str) -> Vec<ConfigParameter> {
    [
        (touch_key(instance), touch),
        (capture_key(instance), capture),
    ]
    .into_iter()
    .map(|(key, value)| ConfigParameter {
        key,
        value: FactScalar::String(value.to_owned()),
        source: ConfigParameterSource::Explicit,
        settable: false,
    })
    .collect()
}

fn manifest(rows: Vec<ConfigParameter>) -> RuntimeConfigManifest {
    let mut parameters = rows;
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A fake device registered with these backends; its opens report a passed native open.
fn device(touch: &str, capture: &str) -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    *state.backends.lock().expect("fake backends") = Some((touch.to_owned(), capture.to_owned()));
    state
}

/// One instance of a test: its alias, id, fake device and file-layer backends.
struct Node {
    alias: &'static str,
    id: InstanceId,
    state: Arc<FakeState>,
}

impl Node {
    fn new(alias: &'static str, touch: &str, capture: &str) -> (Self, Vec<ConfigParameter>) {
        let id = instance_id();
        (
            Self {
                alias,
                id,
                state: device(touch, capture),
            },
            backend_rows(id, touch, capture),
        )
    }

    /// The switches the instance's registration took: (touch, capture, input opens before).
    fn switches(&self) -> Vec<(String, String, usize)> {
        self.state.switches.lock().expect("fake switches").clone()
    }

    fn backends(&self) -> Option<(String, String)> {
        self.state.backends.lock().expect("fake backends").clone()
    }

    fn opens(&self) -> usize {
        self.state.open_count.load(Ordering::Acquire)
    }
}

/// A host of `nodes` over `rows`, its provider with a bound ADB endpoint per instance (so a
/// start prepares each, as configured instances are), on the real Runtime clock.
fn start(root: &TempDir, nodes: &[&Node], rows: Vec<ConfigParameter>) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_config_manifest(manifest(rows)),
        Arc::new(provider(nodes).with_device_endpoints()),
    )
    .expect("runtime host")
}

fn provider(nodes: &[&Node]) -> FakeProvider {
    FakeProvider::from_entries(
        nodes
            .iter()
            .map(|node| (node.alias.to_owned(), node.id, Arc::clone(&node.state))),
    )
}

fn set(key: String, value: &str) -> ConfigSettingChange {
    ConfigSettingChange {
        key,
        value: Some(json!(value)),
    }
}

/// The switch of `node` to these backends, as `actingctl config set` sends it.
fn switch_request(
    ids: &IdentifierIssuer,
    node: &Node,
    touch: &str,
    capture: &str,
) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::ConfigureSettings {
            changes: vec![
                set(touch_key(node.id), touch),
                set(capture_key(node.id), capture),
            ],
            expected_version: None,
        },
    )
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("the request's receipt")
}

/// The refusal's outcome the request's `command.rejected` carries.
fn refusal(host: &RuntimeHost, request: &RuntimeRequest) -> Outcome {
    host.failure_outcome_for_test(request.request_id())
        .expect("refusal outcomes")
        .expect("the request was refused")
}

fn value<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.value(key).and_then(Value::as_str)
}

/// The codes of the outcome's direct links, in order.
fn causes(outcome: &Outcome) -> Vec<String> {
    outcome
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect()
}

fn assert_refused(
    receipt: &RuntimeReceipt,
    outcome: &Outcome,
    state: RuntimeReceiptState,
    problem: &str,
) {
    assert_eq!(receipt.state(), state, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), "backend_switch_refused");
    assert_eq!(outcome.code().as_str(), "backend_switch_refused");
    assert_eq!(value(outcome, "backend_switch_problem"), Some(problem));
}

fn events(host: &RuntimeHost, query: EventQuery) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(query)
        .expect("ledger events")
}

fn of_correlation(host: &RuntimeHost, request: &RuntimeRequest) -> Vec<PersistedEvent> {
    events(
        host,
        EventQuery {
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        },
    )
}

/// The `device.backend_switched` records so far.
fn switched_records(host: &RuntimeHost) -> Vec<DeviceBackendSwitchedPayload> {
    events(
        host,
        EventQuery {
            event_type: Some(EventType::RuntimeLifecycleObserved),
            ..EventQuery::default()
        },
    )
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::DeviceBackendSwitched(payload)) => {
            Some(payload.clone())
        }
        _ => None,
    })
    .collect()
}

/// The `host.config_overrides` records so far.
fn overrides_records(host: &RuntimeHost) -> usize {
    events(
        host,
        EventQuery {
            event_type: Some(EventType::RuntimeFactRecorded),
            ..EventQuery::default()
        },
    )
    .iter()
    .filter(|event| {
        matches!(
            event.payload(),
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == CONFIG_OVERRIDES_FACT_KEY
        )
    })
    .count()
}

/// The value in effect of `key` and its source, from the Runtime's `config.parameters.instance`.
fn in_effect(host: &RuntimeHost, ids: &IdentifierIssuer, key: &str) -> (String, String) {
    let receipt = send(
        host,
        &runtime_request(ids, RuntimeOperation::RuntimeFactSnapshot),
        4290,
    );
    let Some(RuntimeResult::RuntimeFactSnapshot { snapshot, .. }) = receipt.result() else {
        panic!("expected the runtime fact snapshot: {receipt:?}");
    };
    let rows = snapshot
        .records
        .iter()
        .find(|record| record.key == config_parameters_fact_key("instance"))
        .map(|record| match &record.value {
            ContractFactValue::RecordList(rows) => rows.clone(),
            other => panic!("config.parameters.instance is a record list: {other:?}"),
        })
        .expect("config.parameters.instance");
    let row = rows
        .iter()
        .find(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
        .unwrap_or_else(|| panic!("no row {key}: {rows:?}"));
    let text = |field: &str| match row.get(field) {
        Some(FactScalar::String(text)) => text.clone(),
        other => panic!("{field} of {key}: {other:?}"),
    };
    (text("value"), text("source"))
}

fn assert_healthy(host: &RuntimeHost) {
    assert!(host.fatal_error().expect("runtime health").is_none());
}

#[test]
fn hs2_switch_free_instance_commits_and_records() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let opens_before = a.opens();

    let request = switch_request(&ids, &a, "minitouch", "droidcast_raw");
    let receipt = send(&host, &request, 4201);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::SettingsConfigured { settings, .. }) = receipt.result() else {
        panic!("expected the settings: {receipt:?}");
    };
    assert_eq!(
        settings
            .iter()
            .map(|setting| (setting.key.clone(), setting.source))
            .collect::<Vec<_>>(),
        vec![
            (touch_key(a.id), ConfigParameterSource::Instruction),
            (capture_key(a.id), ConfigParameterSource::Instruction),
        ]
    );
    // The registration took the new backends once, before the new opens; they opened.
    let switches = a.switches();
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(
        (switches[0].0.as_str(), switches[0].1.as_str()),
        ("minitouch", "droidcast_raw")
    );
    assert_eq!(switches[0].2, opens_before);
    assert!(a.opens() > opens_before, "the new backends were opened");
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("minitouch".to_owned(), "instruction".to_owned())
    );
    assert_eq!(
        in_effect(&host, &ids, &capture_key(a.id)),
        ("droidcast_raw".to_owned(), "instruction".to_owned())
    );
    // The records, in order: received, host.config_overrides, config.parameters.instance,
    // device.backend_switched, validated. The command's records and the switch record carry the
    // request's links; the two configuration facts are the Runtime's own, between them.
    let correlated = of_correlation(&host, &request);
    let everything = events(&host, EventQuery::default());
    let position = |recorded: &[PersistedEvent], matches: &dyn Fn(&PersistedEvent) -> bool| {
        recorded
            .iter()
            .filter(|event| matches(event))
            .map(PersistedEvent::sequence)
            .collect::<Vec<_>>()
    };
    let received = position(&correlated, &|event: &PersistedEvent| {
        event.event_type() == EventType::CommandReceived
    });
    let validated = position(&correlated, &|event: &PersistedEvent| {
        event.event_type() == EventType::CommandValidated
    });
    let between = |sequences: Vec<u64>| {
        sequences
            .into_iter()
            .filter(|sequence| {
                received.first() < Some(sequence) && Some(sequence) < validated.first()
            })
            .collect::<Vec<_>>()
    };
    let fact = |key: String| {
        move |event: &PersistedEvent| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                    if recorded.record().key == key
            )
        }
    };
    let switched = position(&correlated, &|event: &PersistedEvent| {
        matches!(
            event.payload(),
            EventPayload::Runtime(RuntimePayload::DeviceBackendSwitched(_))
        )
    });
    assert_eq!(received.len(), 1, "{correlated:#?}");
    assert_eq!(validated.len(), 1, "{correlated:#?}");
    assert_eq!(switched.len(), 1, "{correlated:#?}");
    let overrides = between(position(
        &everything,
        &fact(CONFIG_OVERRIDES_FACT_KEY.to_owned()),
    ));
    let section = between(position(
        &everything,
        &fact(config_parameters_fact_key("instance")),
    ));
    assert_eq!(overrides.len(), 1, "{everything:#?}");
    assert_eq!(section.len(), 1, "{everything:#?}");
    assert!(received[0] < overrides[0]);
    assert!(overrides[0] < section[0]);
    assert!(section[0] < switched[0]);
    assert!(switched[0] < validated[0]);
    let [record]: [DeviceBackendSwitchedPayload; 1] = switched_records(&host)
        .try_into()
        .expect("one switch record");
    assert_eq!(record.instance_alias(), A);
    assert_eq!(
        (record.touch().from.as_str(), record.touch().to.as_str()),
        ("adb_shell_input", "minitouch")
    );
    assert_eq!(
        (record.capture().from.as_str(), record.capture().to.as_str()),
        ("adb", "droidcast_raw")
    );
    assert_eq!(record.nemu_app_index(), None);
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_busy_refused_after_claim_wait() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    // A frozen Runtime clock: the holder's lease never expires while the switch waits.
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_config_manifest(manifest(rows)),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(A);
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4202);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Denied,
        "instance_busy",
    );
    assert_eq!(value(&outcome, "alias"), Some(A));
    let [link] = outcome.causes() else {
        panic!("one cause: {outcome:?}");
    };
    let expired = link.outcome();
    assert_eq!(expired.code().as_str(), "lease_queue_expired");
    assert_eq!(value(expired, "holder_kind"), Some("client_lease"));
    assert_eq!(expired.value("timeout_ms"), Some(&json!(3_000)));
    // Nothing switched, nothing committed.
    assert!(a.switches().is_empty());
    assert!(switched_records(&host).is_empty());
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("adb_shell_input".to_owned(), "explicit".to_owned())
    );
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    assert_healthy(&host);
    drop(holder);
    host.close().expect("close host");
}

#[test]
fn hs2_run_between_steps_is_not_a_boundary() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let package = root.path().join("neutral-task.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write package");
    a.state
        .physical_task_geometry
        .store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest(rows)),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            A,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string())
                .expect("contained task request")
                .with_response_deadline_ms(25)
                .expect("bounded task deadline"),
        ),
    );
    // The run stops inside itself, on its own key, until the switch has its answer; then its
    // deadline passes and it ends.
    let (at_checkpoint, reached) = mpsc::channel();
    let (go_on, resumed) = mpsc::channel::<()>();
    let checkpoint_clock = Arc::clone(&clock);
    host.run_at_contained_task_checkpoint_for_test(run.request_id(), a.id, None, move |_| {
        at_checkpoint.send(()).expect("the test meets the run");
        resumed
            .recv_timeout(MEET)
            .expect("the test lets the run go on");
        checkpoint_clock.advance(25);
    })
    .expect("the run's checkpoint");
    let request = switch_request(&ids, &a, "maatouch", "adb");

    let (receipt, run_receipt) = thread::scope(|scope| {
        let running = scope.spawn(|| send(&host, &run, 4203));
        reached
            .recv_timeout(MEET)
            .expect("the run reached its checkpoint");
        let receipt = send(&host, &request, 4204);
        go_on.send(()).expect("the run goes on");
        (receipt, running.join().expect("the run's thread"))
    });

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Denied,
        "instance_busy",
    );
    let [link] = outcome.causes() else {
        panic!("one cause: {outcome:?}");
    };
    assert_eq!(link.outcome().code().as_str(), "lease_queue_expired");
    assert_eq!(
        value(link.outcome(), "holder_kind"),
        Some("direct_task_run")
    );
    assert!(a.switches().is_empty(), "the run kept its backends");
    assert!(
        matches!(
            run_receipt.state(),
            RuntimeReceiptState::Cancelled | RuntimeReceiptState::Completed
        ),
        "the run went on to its end: {run_receipt:?}"
    );
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_paused_instance_switches() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let pause = runtime_request(
        &ids,
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: A.to_owned(),
            },
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("hs2_check")
                .expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
    );
    assert_eq!(
        send(&host, &pause, 4205).state(),
        RuntimeReceiptState::Completed
    );

    // An operator claim passes the paused instance's closed gate: the switch is the
    // "pause, drain, switch, resume" path of a person who wants it mid-task.
    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4206);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        a.backends(),
        Some(("maatouch".to_owned(), "adb".to_owned()))
    );
    assert_eq!(switched_records(&host).len(), 1);
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_player_hold_refuses() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.hold_player_for_test(A, true)
        .expect("a player holds the instance");

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4207);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Denied,
        "player_held",
    );
    assert_eq!(causes(&outcome), ["player_hold_active"]);
    assert!(a.switches().is_empty());
    assert!(switched_records(&host).is_empty());
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_fenced_instance_refuses() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.fence_instance_for_test(A).expect("fence the instance");
    let opens = a.opens();

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4208);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Denied,
        "instance_fenced",
    );
    assert_eq!(causes(&outcome), ["instance_resources_fenced"]);
    assert!(a.switches().is_empty());
    assert_eq!(a.opens(), opens, "nothing was opened");
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_pending_endpoint_refuses() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // The Runtime holds the instance's emulator stopped: its endpoint is pending.
    host.replace_instance_endpoint_for_test(
        A,
        Some(ResolvedInstanceEndpoint::Pending(PendingAdbEndpoint::new(
            "127.0.0.1",
            DiscoveredInstanceBinding::new(0, "fixture", "1.0.0", "C:/fixture/MuMuManager.exe"),
        ))),
    )
    .expect("hold the instance stopped");
    let opens = a.opens();

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4209);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Denied,
        "endpoint_unavailable",
    );
    assert!(a.switches().is_empty());
    assert_eq!(a.opens(), opens, "nothing was opened");
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_open_failure_keeps_old_and_names_cause() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    a.state
        .failing_touch
        .lock()
        .expect("fake failing touch")
        .insert("maatouch".to_owned());
    let overrides = overrides_records(&host);

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4210);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Failed,
        "open_failed",
    );
    assert_eq!(causes(&outcome), ["input_backend_open_failed"]);
    // The candidate, then the previous backends again, which opened.
    assert_eq!(
        a.switches()
            .into_iter()
            .map(|(touch, capture, _)| (touch, capture))
            .collect::<Vec<_>>(),
        vec![
            ("maatouch".to_owned(), "adb".to_owned()),
            ("adb_shell_input".to_owned(), "adb".to_owned()),
        ]
    );
    assert_eq!(
        a.backends(),
        Some(("adb_shell_input".to_owned(), "adb".to_owned()))
    );
    // Nothing committed: no instruction row, no switch record, the file's value in effect.
    assert_eq!(overrides_records(&host), overrides);
    assert!(switched_records(&host).is_empty());
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("adb_shell_input".to_owned(), "explicit".to_owned())
    );
    // The candidate's failure started no ladder.
    let ladders = events(&host, EventQuery::default())
        .into_iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value))
                    if matches!(value.phase(), RuntimeLifecyclePhase::RecoveryLadderStarted { .. })
            )
        })
        .count();
    assert_eq!(ladders, 0);
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_restore_failure_fences_only_that_instance() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows_a) = Node::new(A, "adb_shell_input", "adb");
    let (b, rows_b) = Node::new(B, "adb_shell_input", "adb");
    let host = start(&root, &[&a, &b], [rows_a, rows_b].concat());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // The device fails every touch backend now: the candidate and the previous one.
    a.state
        .failing_touch
        .lock()
        .expect("fake failing touch")
        .extend(["maatouch".to_owned(), "adb_shell_input".to_owned()]);
    let b_opens = b.opens();

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4211);

    let outcome = refusal(&host, &request);
    assert_refused(
        &receipt,
        &outcome,
        RuntimeReceiptState::Failed,
        "restore_failed",
    );
    assert_eq!(
        causes(&outcome),
        ["input_backend_open_failed", "input_backend_open_failed"],
        "the candidate's failure, then the restore's"
    );
    assert_eq!(
        a.backends(),
        Some(("adb_shell_input".to_owned(), "adb".to_owned())),
        "the registration holds the previous backends, never the candidate"
    );
    // That instance is unavailable: a self-check of it fails as well.
    let check_a = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: A.to_owned(),
        },
    );
    let checked_a = send(&host, &check_a, 4212);
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = checked_a.result() else {
        panic!("expected a self-check: {checked_a:?}");
    };
    assert!(selfcheck.failure_code.is_some(), "{selfcheck:?}");
    // The other instance is untouched and still passes; the Runtime runs on.
    assert_eq!(b.opens(), b_opens, "nothing touched the other instance");
    assert!(b.switches().is_empty());
    let check_b = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: B.to_owned(),
        },
    );
    let checked_b = send(&host, &check_b, 4213);
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = checked_b.result() else {
        panic!("expected a self-check: {checked_b:?}");
    };
    assert!(selfcheck.failure_code.is_none(), "{selfcheck:?}");
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_persist_failure_rolls_back() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.fail_next_config_persist_for_test()
        .expect("the ledger refuses the next host.config_overrides");

    let request = switch_request(&ids, &a, "maatouch", "adb");
    let receipt = send(&host, &request, 4214);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        "host_config_overrides_persist_failed"
    );
    // The candidate opened, then the previous backends came back and opened again.
    assert_eq!(
        a.switches()
            .into_iter()
            .map(|(touch, capture, _)| (touch, capture))
            .collect::<Vec<_>>(),
        vec![
            ("maatouch".to_owned(), "adb".to_owned()),
            ("adb_shell_input".to_owned(), "adb".to_owned()),
        ]
    );
    assert_eq!(
        a.backends(),
        Some(("adb_shell_input".to_owned(), "adb".to_owned()))
    );
    assert!(switched_records(&host).is_empty());
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("adb_shell_input".to_owned(), "explicit".to_owned())
    );
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_multi_nemu_gate_follows_switch() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows_a) = Node::new(A, "adb_shell_input", "nemu_ipc");
    let (b, rows_b) = Node::new(B, "adb_shell_input", "adb");
    let host = start(&root, &[&a, &b], [rows_a, rows_b].concat());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // One instance on Nemu IPC: it keeps its session at a lease end.
    assert!(host.keeps_device_session_for_test(A).expect("gate of A"));
    let a_opens = a.opens();

    let request = switch_request(&ids, &b, "adb_shell_input", "nemu_ipc");
    let receipt = send(&host, &request, 4215);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );

    // Two on Nemu IPC now: neither keeps its session. The other instance's gate is read again,
    // and nothing is done to its device.
    assert!(!host.keeps_device_session_for_test(A).expect("gate of A"));
    assert!(!host.keeps_device_session_for_test(B).expect("gate of B"));
    assert_eq!(a.opens(), a_opens);
    assert!(a.switches().is_empty());
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_other_instances_untouched() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows_a) = Node::new(A, "adb_shell_input", "adb");
    let (b, rows_b) = Node::new(B, "maatouch", "adb");
    let host = start(&root, &[&a, &b], [rows_a, rows_b].concat());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let b_opens = b.opens();
    let b_closes = b.state.close_count.load(Ordering::Acquire);

    let request = switch_request(&ids, &a, "minitouch", "adb");
    let receipt = send(&host, &request, 4216);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(b.opens(), b_opens, "the other instance opened nothing");
    assert_eq!(
        b.state.close_count.load(Ordering::Acquire),
        b_closes,
        "the other instance closed nothing"
    );
    assert!(b.switches().is_empty());
    assert_eq!(
        b.backends(),
        Some(("maatouch".to_owned(), "adb".to_owned()))
    );
    assert_eq!(
        in_effect(&host, &ids, &touch_key(b.id)),
        ("maatouch".to_owned(), "explicit".to_owned())
    );
    // The switch's records name the switched instance only.
    let [record]: [DeviceBackendSwitchedPayload; 1] = switched_records(&host)
        .try_into()
        .expect("one switch record");
    assert_eq!(record.instance_alias(), A);
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_status_read_not_blocked_during_switch() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let host = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    // The candidate's first open stops until the status read below answered.
    let (entered, opening) = mpsc::channel();
    let (release, released) = mpsc::channel();
    *a.state.open_hold.lock().expect("fake open hold") = Some((entered, released));
    let request = switch_request(&ids, &a, "maatouch", "adb");

    let (snapshot, receipt) = thread::scope(|scope| {
        let switching = scope.spawn(|| send(&host, &request, 4217));
        opening
            .recv_timeout(MEET)
            .expect("the switch is opening the new backends");
        // The settings and the fact snapshot read under the settings' gate, which the switch's
        // device steps do not hold.
        let snapshot = send(
            &host,
            &runtime_request(&ids, RuntimeOperation::RuntimeFactSnapshot),
            4218,
        );
        release.send(()).expect("let the open go on");
        (snapshot, switching.join().expect("the switch's thread"))
    });

    assert_eq!(
        snapshot.state(),
        RuntimeReceiptState::Completed,
        "{snapshot:?}"
    );
    assert!(matches!(
        snapshot.result(),
        Some(RuntimeResult::RuntimeFactSnapshot {
            config_version: Some(_),
            ..
        })
    ));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_restore_at_start_applies_before_preparation() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let host = start(&root, &[&a], rows.clone());
    let request = switch_request(&ids, &a, "minitouch", "adb");
    assert_eq!(
        send(&host, &request, 4219).state(),
        RuntimeReceiptState::Completed
    );
    host.close().expect("close host");

    // A new start over the same ledger and file: the registry is the file's again.
    let state = device("adb_shell_input", "adb");
    let again = Node {
        alias: A,
        id: a.id,
        state,
    };
    let host = start(&root, &[&again], rows);

    // The restored backends reached the registry before the start opened anything.
    let switches = again.switches();
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(
        (
            switches[0].0.as_str(),
            switches[0].1.as_str(),
            switches[0].2
        ),
        ("minitouch", "adb", 0)
    );
    assert!(again.opens() > 0, "the start prepared the instance");
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("minitouch".to_owned(), "instruction".to_owned())
    );
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_restored_backend_failing_open_stays_unavailable() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let host = start(&root, &[&a], rows.clone());
    let request = switch_request(&ids, &a, "minitouch", "adb");
    assert_eq!(
        send(&host, &request, 4220).state(),
        RuntimeReceiptState::Completed
    );
    host.close().expect("close host");

    // The next start restores minitouch, which no longer opens on the device.
    let state = device("adb_shell_input", "adb");
    state
        .failing_touch
        .lock()
        .expect("fake failing touch")
        .insert("minitouch".to_owned());
    let again = Node {
        alias: A,
        id: a.id,
        state,
    };
    let host = start(&root, &[&again], rows);

    // The start kept the restored value and the registry has it; no fall back to the file's.
    assert_eq!(
        again.backends(),
        Some(("minitouch".to_owned(), "adb".to_owned()))
    );
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        ("minitouch".to_owned(), "instruction".to_owned())
    );
    // The instance is unavailable: its self-check fails on the restored backend.
    let check = runtime_request(
        &ids,
        RuntimeOperation::SelfCheckInstance {
            instance_alias: A.to_owned(),
        },
    );
    let checked = send(&host, &check, 4221);
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = checked.result() else {
        panic!("expected a self-check: {checked:?}");
    };
    assert!(selfcheck.failure_code.is_some(), "{selfcheck:?}");
    assert!(
        again
            .switches()
            .iter()
            .all(|(touch, _, _)| touch == "minitouch"),
        "never switched back silently"
    );
    assert_healthy(&host);
    host.close().expect("close host");
}

#[test]
fn hs2_capabilities_follow_switch() {
    use actingcommand_device::{
        AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceTarget, MaaTouchConfig,
        TouchBackendChoice, TouchBackendConfig,
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    let target = DeviceTarget::default();
    let registry = ExecutionBackendRegistry::new([ExecutionBackendRegistration::new(
        A,
        instance_id(),
        "neutral.application",
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(TouchBackendChoice::AdbShellInput),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::Adb),
    )
    .expect("registration")])
    .expect("registry");
    let key_supported = |registry: &ExecutionBackendRegistry| {
        registry
            .resolve(A)
            .expect("registered")
            .capabilities()
            .expect("capabilities")
            .evidence(actingcommand_contract::EmulatorCapability::InputKey)
            .implementation()
            == Some(actingcommand_contract::EmulatorCapabilityImplementation::Supported)
    };
    let backends = |registry: &ExecutionBackendRegistry| {
        let resolved = registry.resolve(A).expect("registered");
        let configuration = resolved.configuration().expect("configuration");
        (
            configuration.input_backend.clone(),
            configuration.capture_backend.clone(),
        )
    };
    assert!(!key_supported(&registry), "adb input has no key");

    // MaaTouch has keys: the table follows the switch, and so does the configuration.
    registry
        .switch_instance_backends(A, TouchBackendChoice::MaaTouch, CaptureBackendChoice::Adb)
        .expect("switch to maatouch");
    assert!(key_supported(&registry));
    assert_eq!(
        backends(&registry),
        ("maatouch".to_owned(), "adb".to_owned())
    );

    // The registration's own check still refuses Nemu touch beside another capture, and the
    // entry keeps what it had.
    registry
        .switch_instance_backends(A, TouchBackendChoice::NemuIpc, CaptureBackendChoice::Adb)
        .expect_err("Nemu touch beside adb capture");
    assert!(key_supported(&registry));
    assert_eq!(
        backends(&registry),
        ("maatouch".to_owned(), "adb".to_owned())
    );

    // The Nemu pair: no key on the Nemu input.
    registry
        .switch_instance_backends(
            A,
            TouchBackendChoice::NemuIpc,
            CaptureBackendChoice::NemuIpc,
        )
        .expect("switch to the Nemu pair");
    assert!(!key_supported(&registry));
    assert_eq!(
        backends(&registry),
        ("nemu_ipc".to_owned(), "nemu_ipc".to_owned())
    );
}
