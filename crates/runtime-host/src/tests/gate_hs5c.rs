// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-5c (register §1.278; design `rd-1031/backend-switch/model-hs5.md` §4.5, §5.2 and §6
//! row HS-5c, with the coordinator's answer to its §8 Q3; coordinator work order
//! `rd-1031/backend-switch/brief-hs-5c.md`): a backend switch is the remedy for a fence that the
//! instance's own backend open caused (`input_backend_open_failed`, `capture_backend_open_failed`,
//! `paired_backend_open_failed`), so such a fence lets the switch through. The candidate's
//! passing self-check lifts the fence on the existing preparation path, the lift carrying the
//! switch's request, and the switch commits; a failing candidate whose restore fails too leaves
//! the fence, its wait for a person and its rechecks as they were. Every other cause still
//! refuses the switch: `gate_hs2::hs2_fenced_instance_refuses`, unchanged, is the counter-case.
//!
//! The device is HS-2's fake provider: a switch replaces its registered backends, and an input
//! open fails while its touch backend is one a test names. The fence is entered as a failure of
//! the instance's own work with the open's code enters it, and handed to a person as the lease
//! sweep hands a fence no ladder may take (the instance's stuck recovery is off). The Runtime
//! clock is a manual one that only the test moves, by less than one recheck interval, so no
//! recheck runs. No device, adb or emulator is touched; the instance ids are minted and the
//! names are neutral. Nothing here sleeps or waits.

use super::gate_403::status_fence;
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::DeviceSelfCheckFailure;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, InstanceStuckRecovery,
    RecoveryTriggerStage, RuntimeConfigManifest, RuntimeLifecyclePhase, RuntimePayload,
    config_parameters_fact_key,
};
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use serde_json::{Value, json};

const A: &str = "node.a";
const B: &str = "node.b";

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

/// One instance of a test: its alias, id and fake device, registered with its file backends.
struct Node {
    alias: &'static str,
    id: InstanceId,
    state: Arc<FakeState>,
}

impl Node {
    /// The node and its file layer's backend rows; its opens report a passed native open.
    fn new(alias: &'static str, touch: &str, capture: &str) -> (Self, Vec<ConfigParameter>) {
        let id = instance_id();
        let state = Arc::new(FakeState::default());
        state.observed_open.store(true, Ordering::Release);
        *state.backends.lock().expect("fake backends") =
            Some((touch.to_owned(), capture.to_owned()));
        (Self { alias, id, state }, backend_rows(id, touch, capture))
    }

    /// The (touch, capture) pairs the instance's registration took, in order.
    fn switches(&self) -> Vec<(String, String)> {
        self.state
            .switches
            .lock()
            .expect("fake switches")
            .iter()
            .map(|(touch, capture, _)| (touch.clone(), capture.clone()))
            .collect()
    }

    fn backends(&self) -> Option<(String, String)> {
        self.state.backends.lock().expect("fake backends").clone()
    }

    fn opens(&self) -> usize {
        self.state.open_count.load(Ordering::Acquire)
    }
}

fn pair(touch: &str, capture: &str) -> (String, String) {
    (touch.to_owned(), capture.to_owned())
}

/// A host of `nodes` over `rows` on a manual Runtime clock that only the test moves. The stuck
/// recovery of `A` is off: no ladder takes its fence, which waits for a person.
fn start(
    root: &TempDir,
    nodes: &[&Node],
    rows: Vec<ConfigParameter>,
) -> (RuntimeHost, Arc<ManualRuntimeClock>) {
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(BTreeMap::from([(
                A.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )]))
            .with_config_manifest(manifest(rows)),
        Arc::new(FakeProvider::from_entries(nodes.iter().map(|node| {
            (node.alias.to_owned(), node.id, Arc::clone(&node.state))
        }))),
    )
    .expect("runtime host");
    (host, clock)
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("the request's receipt")
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
                ConfigSettingChange {
                    key: touch_key(node.id),
                    value: Some(json!(touch)),
                },
                ConfigSettingChange {
                    key: capture_key(node.id),
                    value: Some(json!(capture)),
                },
            ],
            expected_version: None,
        },
    )
}

/// `node`'s two backend keys back to the file's values, as `actingctl config clear` sends it.
fn clear_request(ids: &IdentifierIssuer, node: &Node) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::ConfigureSettings {
            changes: vec![
                ConfigSettingChange {
                    key: touch_key(node.id),
                    value: None,
                },
                ConfigSettingChange {
                    key: capture_key(node.id),
                    value: None,
                },
            ],
            expected_version: None,
        },
    )
}

/// The value in effect of `key` and its source, from the Runtime's `config.parameters.instance`.
fn in_effect(host: &RuntimeHost, ids: &IdentifierIssuer, key: &str) -> (String, String) {
    let receipt = send(
        host,
        &runtime_request(ids, RuntimeOperation::RuntimeFactSnapshot),
        5390,
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

/// The codes of the outcome's direct links, in order.
fn causes(outcome: &Outcome) -> Vec<String> {
    outcome
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect()
}

fn assert_healthy(host: &RuntimeHost) {
    assert!(host.fatal_error().expect("runtime health").is_none());
}

/// The instance ran on its Nemu pair by instruction (its file says ADB), its paired open failed
/// and fenced it, and the fence waits for a person, as on production on 11-04 (board #402
/// 6621962566). `config clear` switches it back to the file's backends: the fence lets the
/// switch through, the candidate's self-check passes and lifts the fence (one
/// `resource_fence_lifted` of `instance_resources_fence_lifted`, carried by the clear's request,
/// its proof the candidate's own preparation, before the switch's record), the wait for a person
/// ends with it, and the switch commits. The other instance, on Nemu capture as every production
/// instance is (so the multi-Nemu gate keeps no session), is untouched.
#[test]
fn hs5_switch_passes_open_failure_fence_and_lifts() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows_a) = Node::new(A, "adb_shell_input", "adb");
    let (b, rows_b) = Node::new(B, "adb_shell_input", "nemu_ipc");
    let (host, _clock) = start(&root, &[&a, &b], [rows_a, rows_b].concat());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let to_nemu = switch_request(&ids, &a, "nemu_ipc", "nemu_ipc");
    let switched = send(&host, &to_nemu, 5301);
    assert_eq!(
        switched.state(),
        RuntimeReceiptState::Completed,
        "{switched:?}"
    );
    host.fence_instance_awaiting_person_for_test(
        A,
        DeviceSelfCheckFailure::PairedBackendOpenFailed.code(),
    )
    .expect("fence the instance");
    let fence = status_fence(&host, A).expect("the instance is fenced");
    assert_eq!(
        fence.cause.as_str(),
        DeviceSelfCheckFailure::PairedBackendOpenFailed
            .code()
            .as_str()
    );
    assert!(
        fence.awaiting_person && fence.person_wait.is_some(),
        "the fence waits for a person: {fence:?}"
    );
    let b_opens = b.opens();

    let clear = clear_request(&ids, &a);
    let receipt = send(&host, &clear, 5302);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    // The file's backends are in effect, and the registration took them.
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        pair("adb_shell_input", "explicit")
    );
    assert_eq!(
        in_effect(&host, &ids, &capture_key(a.id)),
        pair("adb", "explicit")
    );
    assert_eq!(a.backends(), Some(pair("adb_shell_input", "adb")));
    // The fence is lifted, and its wait for a person ended with it.
    assert_eq!(status_fence(&host, A), None, "the fence is lifted");
    let events = all_events(&host);
    let lifts = phases(&events)
        .into_iter()
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::ResourceFenceLifted {
                instance_id,
                preparation,
                outcome,
            } => Some((event, instance_id, preparation.sequence, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(lifted, lifted_instance, proof_sequence, outcome)] = lifts.as_slice() else {
        panic!("one lift record: {lifts:#?}");
    };
    assert_eq!(*lifted_instance, a.id);
    assert_eq!(
        outcome.code().as_str(),
        HostCode::InstanceResourcesFenceLifted.as_str()
    );
    assert_eq!(lifted.links().request_id(), Some(&clear.request_id()));
    let proof = events
        .iter()
        .find(|event| event.sequence() == *proof_sequence)
        .expect("the proving preparation");
    assert_eq!(proof.links().request_id(), Some(&clear.request_id()));
    assert!(
        matches!(
            phases(std::slice::from_ref(proof))
                .first()
                .map(|(_, phase)| phase.clone()),
            Some(RuntimeLifecyclePhase::InstancePreparationFinished {
                stage: RecoveryTriggerStage::ConnectionPreparation,
                outcome: None,
                ..
            })
        ),
        "the candidate's passing preparation proves the close: {proof:#?}"
    );
    // The switch committed after the lift: one record of the clear, back to the file's backends.
    let records = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::DeviceBackendSwitched(record))
                if event.links().request_id() == Some(&clear.request_id()) =>
            {
                Some((event.sequence(), record.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(switched_at, record)] = records.as_slice() else {
        panic!("one switch record of the clear: {records:#?}");
    };
    assert!(
        lifted.sequence() < *switched_at,
        "the fence is lifted before the switch commits"
    );
    assert_eq!(
        (record.touch().from.as_str(), record.touch().to.as_str()),
        ("nemu_ipc", "adb_shell_input")
    );
    assert_eq!(
        (record.capture().from.as_str(), record.capture().to.as_str()),
        ("nemu_ipc", "adb")
    );
    // The other instance is untouched (#419).
    assert_eq!(b.opens(), b_opens, "nothing opened the other instance");
    assert!(b.switches().is_empty());
    assert_eq!(status_fence(&host, B), None);
    assert_healthy(&host);
    host.close().expect("close host");
}

/// The instance runs on another touch backend by instruction; that backend's open failed and
/// fenced it (the second of the three causes), and the fence waits for a person. Now neither the
/// file's touch backend nor the instruction's opens: `config clear` passes the fence, its
/// candidate fails, the previous backends come back and fail again (`restore_failed`, the
/// candidate's failure then the restore's). Nothing is committed, nothing lifts the fence, and
/// the fence, its wait for a person, its rechecks and their times are exactly as they were.
///
/// The design's §5.2 row names `open_failed`; that receipt means the previous backends opened
/// again, whose passing self-check lifts the fence (§4.5), so a candidate that fails while the
/// fence stays is `restore_failed`.
#[test]
fn hs5_switch_back_failing_keeps_fence() {
    let root = TempDir::new().expect("tempdir");
    let (a, rows) = Node::new(A, "adb_shell_input", "adb");
    let (host, clock) = start(&root, &[&a], rows);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let to_maatouch = switch_request(&ids, &a, "maatouch", "adb");
    let switched = send(&host, &to_maatouch, 5303);
    assert_eq!(
        switched.state(),
        RuntimeReceiptState::Completed,
        "{switched:?}"
    );
    host.fence_instance_awaiting_person_for_test(
        A,
        DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
    )
    .expect("fence the instance");
    // Time passes within the recheck interval: a wait begun again would show it.
    clock.advance(1_000);
    let before = status_fence(&host, A).expect("the instance is fenced");
    assert!(
        before.awaiting_person && before.person_wait.is_some(),
        "the fence waits for a person: {before:?}"
    );
    a.state
        .failing_touch
        .lock()
        .expect("fake failing touch")
        .extend(["adb_shell_input".to_owned(), "maatouch".to_owned()]);

    let clear = clear_request(&ids, &a);
    let receipt = send(&host, &clear, 5304);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), "backend_switch_refused");
    let outcome = host
        .failure_outcome_for_test(clear.request_id())
        .expect("refusal outcomes")
        .expect("the clear was refused");
    assert_eq!(
        outcome
            .value("backend_switch_problem")
            .and_then(Value::as_str),
        Some("restore_failed")
    );
    assert_eq!(
        causes(&outcome),
        ["input_backend_open_failed", "input_backend_open_failed"],
        "the candidate's failure, then the restore's"
    );
    // The candidate, then the previous backends again; the registration holds the previous ones.
    assert_eq!(
        a.switches(),
        [
            pair("maatouch", "adb"),
            pair("adb_shell_input", "adb"),
            pair("maatouch", "adb"),
        ]
    );
    assert_eq!(a.backends(), Some(pair("maatouch", "adb")));
    // Nothing committed: the instruction's values stay in effect.
    assert_eq!(
        in_effect(&host, &ids, &touch_key(a.id)),
        pair("maatouch", "instruction")
    );
    // The fence stays as it was: its cause and since, its wait for a person, the rechecks made
    // and the next one's time.
    assert_eq!(status_fence(&host, A), Some(before));
    assert!(
        !phases(&all_events(&host))
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::ResourceFenceLifted { .. })),
        "nothing lifted the fence"
    );
    assert_healthy(&host);
    host.close().expect("close host");
}
