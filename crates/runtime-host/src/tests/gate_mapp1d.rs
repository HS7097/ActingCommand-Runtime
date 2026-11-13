// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1d (register §1.359; `contracts/device-group.md`, "The monitor's probe in a
//! device group", "Device-level actions", row 7, and "One queue, one worker, one lease per
//! device"): the three group gaps MAPP-1c's review found, closed, through the host: the monitor's
//! probe of a member, the hand-over at a lapsed run lease's expiry, and the queue's grant judged
//! by one member's gate; and (fix round 1) a member's failed probe capture beside another,
//! held member's queued claim, which must not end the Runtime.
//!
//! Two fixture instances A and B are one device group of several (`device.one`) and C is alone on
//! its own device. The load refuses every group of several (actingd's MAPP-1 gate), so the host is
//! handed the member → device key map and the group's name as the assembly builds them. A and B
//! write one fake device's session log: every capture or input backend each opens or closes, in
//! the order the device saw them; every capture close of theirs must run under a fenced
//! resource-close step. A manual Runtime clock whose monotonic time moves only when a test (or a
//! run's own wait) moves it; stuck recovery off, so a fence waits for a person and no ladder
//! acts. The resident monitor loop is held, and each probe round is run by the test itself, as the
//! loop runs it; nothing sleeps. Every expected value is the contract's or the coordinator's
//! ruling; every comparison is within one host, the lone instance's records by payloads with
//! every id, time, registry place and evaluation `seed` normalised, never by an identity. Neutral
//! names only.

use super::gate_386_sr4a3::{once as blackout_once, silence_manifest};
use super::gate_403::status_fence;
use super::instance_workers::{all_events, phases};
use super::*;
use crate::host::{ContainedTaskCheckpointPoint, HostClaimForTest};
use actingcommand_contract::{
    HolderId, InstanceStuckRecovery, ResourceQuiescence, RuntimeLifecyclePhase,
    RuntimeMonitorState, RuntimePayload, SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_scheduler::codes::SchedulerCode;
use actingcommand_scheduler::{ClaimKind, QueuedLease};
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
const GROUP: &str = "device.one";
/// Bounds a broken run's wait only; nothing sleeps.
const GATE_WAIT: Duration = Duration::from_secs(120);
/// The lease time `request_host_claim_for_test` gives every claim it asks for.
const CLAIM_TTL_MS: u64 = 60_000;
/// The monitor's interval: the longest the policy takes, so a probe that ran is not due again in
/// a test, and one that did not run stays due.
const MONITOR_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
/// How far a test moves the Runtime clock to let a lease lapse: past a direct run's lease (its
/// response deadline and the heartbeat reserve), past the run's end grace
/// (`host_claims.run_end_grace_ms`) and past a client lease's time.
const LAPSE_MS: u64 = 60 * 60 * 1_000;

// ---- The fixture.

/// A host whose instances A and B are the device group `GROUP` and C is alone, on a manual
/// Runtime clock whose monotonic time starts at 0, with its resident monitor loop held.
struct Device {
    host: Arc<RuntimeHost>,
    clock: Arc<ManualRuntimeClock>,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    log: DeviceSessionLog,
    next_connection: AtomicU64,
    root: TempDir,
}

impl Device {
    fn start() -> Self {
        Self::start_with_blackout_of(None)
    }

    /// [`Self::start`], and when `blackout` names a member, a blackout of that member's own list
    /// (`scheduling-blackout.md`) that covers the Runtime clock's time from the start, for an hour
    /// and more: its routine gate is silenced, as `routine_gate` reads it.
    fn start_with_blackout_of(blackout: Option<&'static str>) -> Self {
        let root = TempDir::new().expect("tempdir");
        let ids = BTreeMap::from([(A, instance_id()), (B, instance_id()), (C, instance_id())]);
        let device = ids[A].min(ids[B]);
        let states = [A, B, C]
            .into_iter()
            .map(|alias| (alias, Arc::new(FakeState::default())))
            .collect::<BTreeMap<_, _>>();
        let log = DeviceSessionLog::default();
        for member in [A, B] {
            states[member].share_session_log(member, &log);
            states[member]
                .require_fenced_capture_close
                .store(true, Ordering::Release);
        }
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(
                [A, B, C]
                    .into_iter()
                    .map(|alias| {
                        (
                            alias.to_owned(),
                            InstanceStuckRecovery {
                                enabled: false,
                                cooldown_secs: 600,
                            },
                        )
                    })
                    .collect(),
            )
            .with_device_keys(BTreeMap::from([(ids[A], device), (ids[B], device)]))
            .with_device_group_names(BTreeMap::from([
                (A.to_owned(), GROUP.to_owned()),
                (B.to_owned(), GROUP.to_owned()),
            ]));
        let config = match blackout {
            Some(member) => config.with_config_manifest(silence_manifest(
                json!({}),
                json!([]),
                &[(
                    ids[member],
                    json!([blackout_once(
                        "mapp1d.member",
                        POLICY_NOW_UNIX_MS - 1_000,
                        POLICY_NOW_UNIX_MS + LAPSE_MS,
                    )]),
                )],
            )),
            None => config,
        };
        let entries = states
            .iter()
            .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
            .collect::<Vec<_>>();
        let host = RuntimeHost::start(config, Arc::new(FakeProvider::from_entries(entries)))
            .expect("start the device group's host");
        host.hold_monitor_loop_for_test()
            .expect("hold the resident monitor loop");
        Self {
            host: Arc::new(host),
            clock,
            ids,
            states,
            log,
            next_connection: AtomicU64::new(2_000),
            root,
        }
    }

    fn connection(&self) -> ConnectionId {
        ConnectionId::new(self.next_connection.fetch_add(1, Ordering::AcqRel))
            .expect("a test connection")
    }

    fn request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        runtime_request(&IdentifierIssuer::new().expect("issuer"), operation)
    }

    /// `operation` processed as a client's, on a connection of its own.
    fn process(&self, operation: RuntimeOperation) -> RuntimeReceipt {
        self.host
            .process_request_for_test(&self.request(operation), self.connection())
            .expect("the request's receipt")
    }

    /// The business capacity sampled now, as the capacity monitor samples it while time passes.
    fn sample_capacity(&self) {
        let sample = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample().expect("capacity sample");
    }

    /// `alias`'s monitor configured: one probe per `MONITOR_INTERVAL_MS` of the page `home`, its
    /// recovery asked for when `recovery`. It is due at once.
    fn configure_monitor(&self, alias: &str, recovery: bool) {
        let receipt = self.process(RuntimeOperation::ConfigureMonitor {
            instance_alias: alias.to_owned(),
            policy: RuntimeMonitorPolicy::new(MONITOR_INTERVAL_MS, "home", recovery)
                .expect("monitor policy"),
        });
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    fn clear_monitor(&self, alias: &str) {
        let receipt = self.process(RuntimeOperation::ClearMonitor {
            instance_alias: alias.to_owned(),
        });
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// `alias`'s monitor state, as the monitor's status reads it.
    fn monitor_state(&self, alias: &str) -> Option<RuntimeMonitorState> {
        let receipt = self.process(RuntimeOperation::MonitorStatus);
        let Some(RuntimeResult::MonitorStatus { status }) = receipt.result() else {
            panic!("the monitor's status: {receipt:?}");
        };
        status
            .instances()
            .iter()
            .find(|row| row.instance_alias() == alias)
            .and_then(|row| row.state().cloned())
    }

    /// One round of `alias`'s monitor probe, as the resident loop runs it; whether it was due.
    fn probe(&self, alias: &str) -> bool {
        self.host
            .run_due_monitor_probe_for_test(alias)
            .expect("a probe round")
    }

    /// One probe round of `alias` whose capture waits in the fake while `during` runs, so
    /// `during` acts while the probe holds the device's admission guard with the member's capture
    /// open. Returns whether the probe was due and what `during` returned.
    fn probe_while<T>(&self, alias: &'static str, during: impl FnOnce() -> T) -> (bool, T) {
        let (entered_tx, entered) = mpsc::channel::<()>();
        let (proceed_tx, proceed) = mpsc::channel::<()>();
        *self.states[alias]
            .capture_hook
            .lock()
            .expect("capture hook") = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = proceed.recv_timeout(GATE_WAIT);
        }));
        thread::scope(|scope| {
            let prober = scope.spawn(|| self.probe(alias));
            entered
                .recv_timeout(GATE_WAIT)
                .expect("the member's probe capture is in flight");
            let during = during();
            proceed_tx.send(()).expect("the capture goes on");
            (prober.join().expect("the prober"), during)
        })
    }

    /// One keyless read-only observation of `alias`.
    fn observe(&self, alias: &str) -> RuntimeReceipt {
        self.process(RuntimeOperation::ObserveReadonly {
            instance_alias: alias.to_owned(),
        })
    }

    /// A keyless observation of `alias` whose capture waits in the fake while `during` runs.
    fn observe_while<T>(
        &self,
        alias: &'static str,
        during: impl FnOnce() -> T,
    ) -> (RuntimeReceipt, T) {
        let (entered_tx, entered) = mpsc::channel::<()>();
        let (proceed_tx, proceed) = mpsc::channel::<()>();
        *self.states[alias]
            .capture_hook
            .lock()
            .expect("capture hook") = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = proceed.recv_timeout(GATE_WAIT);
        }));
        thread::scope(|scope| {
            let observer = scope.spawn(|| self.observe(alias));
            entered
                .recv_timeout(GATE_WAIT)
                .expect("the member's capture is in flight");
            let during = during();
            proceed_tx.send(()).expect("the capture goes on");
            (observer.join().expect("the observer"), during)
        })
    }

    /// A Runtime claim of `kind` on `alias` (60 s of lease time).
    fn claim(&self, alias: &str, kind: ClaimKind) -> HostClaimForTest {
        self.host
            .request_host_claim_for_test(alias, kind)
            .expect("a claim")
    }

    fn release(&self, claim: &HostClaimForTest, token: &LeaseToken) {
        self.host
            .release_host_claim_for_test(claim, token)
            .expect("the claim's lease end");
    }

    /// A scheduling pause of `alias`, as a client asks for it; it holds nothing yet.
    fn pause(&self, alias: &str) {
        let mut client = TestClient::connect(&self.host);
        let pause = client.request(RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: alias.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("mapp1d_member_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        });
        let receipt = client.send(&pause);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// The device's session log so far, as the device saw it.
    fn session_log(&self) -> Vec<(String, bool)> {
        self.log.lock().expect("the device's session log").clone()
    }

    /// The device's session log so far; at every step of it at most one member has a session
    /// open, and no member closes more than it opened.
    fn sessions(&self) -> Vec<(String, bool)> {
        let entries = self.session_log();
        let mut open = BTreeMap::<String, usize>::new();
        for (step, (member, opened)) in entries.iter().enumerate() {
            let count = open.entry(member.clone()).or_default();
            *count = if *opened {
                *count + 1
            } else {
                count.checked_sub(1).unwrap_or_else(|| {
                    panic!("step {step}: {member} closed more than it opened: {entries:?}")
                })
            };
            let members_open = open.values().filter(|count| **count > 0).count();
            assert!(
                members_open <= 1,
                "step {step}: two members' sessions open on the device at once: {entries:?}"
            );
        }
        entries
    }

    /// Every record of the request `request_id`, in ledger order.
    fn records_of(&self, request_id: actingcommand_contract::RequestId) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            })
            .expect("the request's records")
    }

    /// Every record after `after`, in ledger order.
    fn events_after(&self, after: u64) -> Vec<PersistedEvent> {
        all_events(&self.host)
            .into_iter()
            .filter(|event| event.sequence() > after)
            .collect()
    }

    /// The ledger's last sequence.
    fn head(&self) -> u64 {
        all_events(&self.host)
            .last()
            .map_or(0, PersistedEvent::sequence)
    }

    /// Every record of `query`, in ledger order, as the lone instance's comparison reads them
    /// ([`comparable`]), but a frame's retention records, which the frame cleaner writes on its
    /// own time.
    fn comparable_records(&self, query: EventQuery) -> Vec<Compared> {
        let events = self
            .host
            .query_persisted_events_for_test(query)
            .expect("records")
            .into_iter()
            .filter(|event| event.payload().artifact_retention().is_none())
            .collect::<Vec<_>>();
        comparable(&events)
    }

    /// How many `runtime.failed` records name `alias`.
    fn failures_of(&self, alias: &str) -> usize {
        all_events(&self.host)
            .iter()
            .filter(|event| {
                event.event_type() == EventType::RuntimeFailed
                    && event.links().instance_id() == Some(&self.ids[alias])
            })
            .count()
    }

    /// How many leases were granted to `alias` (its `lease.granted` records).
    fn grants_to(&self, alias: &str) -> usize {
        all_events(&self.host)
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseGranted
                    && event.links().instance_id() == Some(&self.ids[alias])
            })
            .count()
    }

    /// How many records of a monitor probe of `alias` (its request, start, end, recovery
    /// coordination, capture and recognition) the ledger holds.
    fn probe_records(&self, alias: &str) -> usize {
        all_events(&self.host)
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type(),
                    EventType::MonitorProbeRequested
                        | EventType::MonitorProbeStarted
                        | EventType::MonitorProbeCompleted
                        | EventType::MonitorProbeFailed
                        | EventType::MonitorRecoveryAdmitted
                        | EventType::MonitorRecoveryDeferred
                        | EventType::CaptureRequested
                        | EventType::CaptureCompleted
                        | EventType::RecognitionRequested
                        | EventType::RecognitionCompleted
                ) && event.links().instance_id() == Some(&self.ids[alias])
            })
            .count()
    }

    /// The neutral package of a direct run written beside the host, the fake device of `alias`
    /// set to finish it on its target page.
    fn run_package(&self, alias: &str) -> String {
        let package = self.root.path().join("neutral-task.zip");
        fs::write(&package, neutral_contained_task_package(true)).expect("write package");
        let state = &self.states[alias];
        state.physical_task_geometry.store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        package.display().to_string()
    }

    fn close(self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime keeps running"
        );
        let Ok(host) = Arc::try_unwrap(self.host) else {
            panic!("a hook still holds the host");
        };
        host.close().expect("close the device group's host");
    }
}

/// The holder a test claim asked for.
fn holder(claim: &HostClaimForTest) -> HolderId {
    match claim.request.operation() {
        RuntimeOperation::AcquireLease { holder_id, .. } => *holder_id,
        other => panic!("a claim's request: {other:?}"),
    }
}

/// The sequence of the first confirmed resource close of `instance` in `events`.
fn first_close(events: &[PersistedEvent], instance: InstanceId) -> u64 {
    phases(events)
        .into_iter()
        .find_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id,
                quiescence: ResourceQuiescence::Confirmed,
                ..
            } if instance_id == instance => Some(event.sequence()),
            _ => None,
        })
        .expect("a confirmed close of the member's session")
}

/// The sequence of the `lease.granted` of the claim `claim`.
fn grant_of(events: &[PersistedEvent], claim: &HostClaimForTest) -> u64 {
    events
        .iter()
        .find(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.links().request_id() == Some(&claim.request.request_id())
        })
        .map(PersistedEvent::sequence)
        .expect("the claim's lease.granted")
}

/// `claim`'s own records show the queue's own grant: it was requested and queued, then admitted
/// and granted, and never handed the key by a transfer.
fn assert_granted_from_the_queue(device: &Device, claim: &HostClaimForTest) {
    let types = device
        .records_of(claim.request.request_id())
        .iter()
        .map(PersistedEvent::event_type)
        .collect::<Vec<_>>();
    assert_events_in_order(
        &types,
        &[
            EventType::LeaseRequested,
            EventType::SchedulerQueued,
            EventType::SchedulerAdmitted,
            EventType::LeaseGranted,
        ],
    );
    assert!(
        !types.contains(&EventType::LeaseTransferred),
        "no transfer handed the claim the key: {types:?}"
    );
}

/// How many of `member`'s backends `log` shows open at its end.
fn open_backends(log: &[(String, bool)], member: &str) -> usize {
    let opened = log
        .iter()
        .filter(|(label, opened)| label == member && *opened)
        .count();
    let closed = log
        .iter()
        .filter(|(label, opened)| label == member && !*opened)
        .count();
    opened.saturating_sub(closed)
}

/// A capture close that ends without confirming that its connection is gone.
fn unconfirmed_close() -> DeviceError {
    DeviceError::fatal("synthetic capture close left its connection unconfirmed")
        .with_resource_summary(
            actingcommand_device::DeviceResourceQuiescence::Unconfirmed,
            1,
        )
}

/// Whether a member is named `key`, whose value is a time or a place: an instant on either clock,
/// a measurement on the process clock, a local day, a ledger position or a record index, a place
/// in the monitor's registry (its revisions and configuration versions, which every change of the
/// registry moves on), or an evaluation's `seed` (a digest of the fact snapshot's id, the
/// evaluation's time and the owner epoch; a member of the dispatch admission's `activity`). Two
/// rounds on one host hold these apart however alike they are.
fn time_or_place(key: &str) -> bool {
    key.ends_with("_unix_ms")
        || key.ends_with("_monotonic_ms")
        || key.ends_with("_us")
        || key.ends_with("_at")
        || matches!(
            key,
            "duration_ms"
                | "local_day"
                | "sequence"
                | "record_index"
                | "input_ledger_position"
                | "revision"
                | "previous_revision"
                | "configuration_version"
                | "probe_configuration_version"
                | "seed"
        )
}

/// `run` (lowercase hexadecimal digits) as [`without_identities`] writes it: a run of 32 or more
/// is an identity and is replaced.
fn push_hex_run(text: &mut String, run: &mut String) {
    if run.len() >= 32 {
        text.push_str("<hex>");
    } else {
        text.push_str(run);
    }
    run.clear();
}

/// `text` with each run of 32 or more lowercase hexadecimal digits replaced: a minted id (a
/// request, lease, holder, frame, instance or causation id), a digest.
fn without_identities(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if matches!(character, '0'..='9' | 'a'..='f') {
            run.push(character);
        } else {
            push_hex_run(&mut written, &mut run);
            written.push(character);
        }
    }
    push_hex_run(&mut written, &mut run);
    written
}

/// `value` with its ids, times and places normalised: every member [`time_or_place`] names is
/// replaced whole, and every string loses its identities ([`without_identities`]).
fn normalised(value: &mut Value) {
    match value {
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                if time_or_place(key) {
                    *member = json!("<normalised>");
                } else {
                    normalised(member);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalised),
        Value::String(text) => *text = without_identities(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// One record as the lone instance's comparison reads it ([`comparable`]).
#[derive(Debug, PartialEq)]
struct Compared {
    event_type: EventType,
    severity: EventSeverity,
    /// The instance the record's links name: for a grant from the queue, the member the queue's
    /// grant names (`pump`'s attribution). One registered instance of one host, never minted.
    instance: Option<InstanceId>,
    /// The whole payload, normalised ([`normalised`]).
    payload: Value,
}

/// The records as the lone instance's comparison reads them: each one's type, severity, the
/// instance its links name and whole payload, normalised ([`normalised`]).
fn comparable(events: &[PersistedEvent]) -> Vec<Compared> {
    events
        .iter()
        .map(|event| {
            let mut payload =
                serde_json::to_value(event.payload()).expect("a record's payload encodes");
            normalised(&mut payload);
            Compared {
                event_type: event.event_type(),
                severity: event.severity(),
                instance: event.links().instance_id().copied(),
                payload,
            }
        })
        .collect()
}

/// Every difference between `with` and `without`, named in one list: the record counts, then each
/// record that differs, with both sides.
fn differences(part: &str, with: &[Compared], without: &[Compared]) -> Vec<String> {
    let mut found = Vec::new();
    if with.len() != without.len() {
        found.push(format!(
            "{part}: {} records on, {} off",
            with.len(),
            without.len()
        ));
    }
    for (index, (on, off)) in with.iter().zip(without).enumerate() {
        if on != off {
            found.push(format!("{part}, record {index}: on {on:?}, off {off:?}"));
        }
    }
    found
}

// ---- The tests.

/// The probe of a member on a free device, during whose capture B's claim queues (it meets the
/// device's admission guard the probe holds). On two hosts started alike, with the test-only
/// switch (compiled under `cfg(test)` only) off and on. Off (the probe as before the slice): the
/// probe keeps A's session, B's claim is granted with A's session open and B's observation under
/// B's lease opens B's session beside it (two members' sessions open, the old gap). On: A's
/// session is closed before the probe lets the device go, under a session close ahead that the
/// waiting claim does not refuse; B's claim is then granted by the queue's own grant (the sweep's
/// tick pumps it; the probe's guard pumps nothing, as before), after A's close, with B's member,
/// B's holder and its own lease time; the probe's verdict is recorded, nothing of A failed, every
/// capture close was fenced, and at no step are two members' sessions open.
#[test]
fn mapp1d_a_members_probe_on_a_free_device_closes_its_session_before_another_members_lease_begins()
{
    for off in [true, false] {
        let device = Device::start();
        device
            .host
            .mapp1d_off_for_test(off)
            .expect("the slice's branches");
        device.configure_monitor(A, false);
        let (probed, claim) = device.probe_while(A, || device.claim(B, ClaimKind::SelfCheck));
        assert!(probed, "A's probe was due and ran");
        assert_eq!(
            claim.queued.as_ref().map(QueuedLease::position),
            Some(1),
            "B's claim waited for the device: {claim:?}"
        );
        device
            .host
            .expire_due_leases_for_test()
            .expect("one sweep tick");
        let token = claim
            .wait_for_grant(GATE_WAIT)
            .expect("the grant slot")
            .expect("B's claim is granted once the probe let the device go");
        assert_eq!(token.instance_id(), device.ids[B]);
        assert_eq!(token.holder_id(), holder(&claim));
        assert_eq!(
            token.expires_at_monotonic_ms(),
            device.clock.monotonic_ms() + CLAIM_TTL_MS,
            "the claim's own lease time from its grant"
        );
        let at_grant = device.session_log();
        let observed_b = device.observe(B);
        assert_eq!(
            observed_b.state(),
            RuntimeReceiptState::Completed,
            "{observed_b:?}"
        );
        if off {
            assert_eq!(
                at_grant,
                vec![(A.to_owned(), true)],
                "off: A's session is still open when B's lease begins"
            );
            assert_eq!(
                device.session_log(),
                vec![(A.to_owned(), true), (B.to_owned(), true)],
                "off: two members' sessions open on the device at once"
            );
        } else {
            assert_eq!(
                at_grant,
                vec![(A.to_owned(), true), (A.to_owned(), false)],
                "on: A's session is closed before B's lease begins"
            );
            assert_eq!(
                device.sessions(),
                vec![
                    (A.to_owned(), true),
                    (A.to_owned(), false),
                    (B.to_owned(), true),
                ],
                "on: B's session opens after A's is closed"
            );
            let events = all_events(&device.host);
            assert!(
                first_close(&events, device.ids[A]) < grant_of(&events, &claim),
                "A's close is recorded before B's lease.granted"
            );
            assert_granted_from_the_queue(&device, &claim);
            let state = device.monitor_state(A).expect("A's monitor state");
            assert_eq!(state.run_count(), 1, "{state:?}");
            assert!(
                state.last_decision().is_some() && state.last_error().is_none(),
                "{state:?}"
            );
            assert_eq!(device.failures_of(A), 0, "nothing of A failed");
            assert_eq!(
                device.states[A]
                    .unfenced_capture_close_count
                    .load(Ordering::Acquire),
                0,
                "A's capture closed under a fenced resource-close step"
            );
        }
        device.release(&claim, &token);
        device.close();
    }
}

/// The probe of a member while another member's lease holds the device: this round is not made,
/// twice. No capture is asked and no session opened, nothing of the probe is recorded (no request,
/// no failure), no lease of A is taken, and A's monitor state is exactly what it was: no run
/// counted, no verdict replaced, and it is still due (the status shows the probe waiting, as a
/// probe a blackout passes over). Once B's lease ends the next round runs: its verdict is
/// recorded, and its session is opened and closed before the probe lets the device go.
#[test]
fn mapp1d_a_members_probe_while_another_member_holds_the_device_waits_and_runs_once_it_is_free() {
    let device = Device::start();
    let holder_claim = device.claim(B, ClaimKind::SelfCheck);
    let token = holder_claim
        .granted
        .clone()
        .unwrap_or_else(|| panic!("B's claim holds the free device: {holder_claim:?}"));
    device.configure_monitor(A, false);
    let before = device.monitor_state(A).expect("A's monitor state");
    assert_eq!(before.run_count(), 0, "{before:?}");
    for round in 0..2 {
        assert!(device.probe(A), "round {round}: the probe is still due");
    }
    assert_eq!(
        device.states[A].capture_count.load(Ordering::Acquire),
        0,
        "no capture asked"
    );
    assert!(device.session_log().is_empty(), "nothing opened");
    assert_eq!(device.probe_records(A), 0, "nothing of the probe recorded");
    assert_eq!(device.failures_of(A), 0, "no failure recorded");
    assert_eq!(device.grants_to(A), 0, "no lease of A");
    assert_eq!(
        device.monitor_state(A),
        Some(before.clone()),
        "A's monitor state is unchanged: no run counted, no verdict replaced, still due"
    );
    assert!(
        device.host.fatal_error().expect("runtime health").is_none(),
        "the Runtime keeps running"
    );

    device.release(&holder_claim, &token);
    assert!(
        device.probe(A),
        "the next round runs once the device is free"
    );
    let after = device.monitor_state(A).expect("A's monitor state");
    assert_eq!(after.run_count(), before.run_count() + 1, "{after:?}");
    assert!(
        after.last_decision().is_some() && after.last_error().is_none(),
        "{after:?}"
    );
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true), (A.to_owned(), false)],
        "A's probe opened its session and closed it before it let the device go"
    );
    assert!(
        !device.probe(A),
        "the probe is due again only one interval later"
    );
    device.close();
}

/// The two ways a member's probe could end the whole Runtime, each driven as far as the fixture
/// allows. (1) Its capture fails: while B's lease holds the device the round is not made (no
/// capture, no close lease of A beside B's lease, the Runtime's health clean); on the free device
/// the capture fails and the probe fails with it (`capture_failed` in A's monitor state), its
/// session closed under a fenced close, and the Runtime's health stays clean. (2) Its verdict asks
/// a recovery: while B's lease holds the device the round is not made (no recovery coordination
/// recorded, the Runtime's health clean); on the free device the probe's recovery coordination
/// reads no lease of another member and records the recovery admitted (`scheduler_available`),
/// and the Runtime's health stays clean.
#[test]
fn mapp1d_neither_former_fatal_path_of_a_members_probe_is_reached() {
    // (1) Its capture fails.
    let device = Device::start();
    device.states[A].fail_capture.store(true, Ordering::Release);
    let holder_claim = device.claim(B, ClaimKind::SelfCheck);
    let token = holder_claim
        .granted
        .clone()
        .unwrap_or_else(|| panic!("B's claim holds the free device: {holder_claim:?}"));
    device.configure_monitor(A, false);
    assert!(device.probe(A), "the probe is due");
    assert_eq!(device.states[A].capture_count.load(Ordering::Acquire), 0);
    assert_eq!(
        device.grants_to(A),
        0,
        "no close lease of A beside B's lease"
    );
    assert!(device.host.fatal_error().expect("runtime health").is_none());
    device.release(&holder_claim, &token);
    assert!(
        device.probe(A),
        "the next round runs once the device is free"
    );
    assert!(device.states[A].capture_count.load(Ordering::Acquire) > 0);
    let state = device.monitor_state(A).expect("A's monitor state");
    assert_eq!(
        state.last_error(),
        Some(RuntimeErrorCode::CaptureFailed),
        "{state:?}"
    );
    let log = device.sessions();
    assert_eq!(open_backends(&log, A), 0, "A's session is closed: {log:?}");
    assert_eq!(
        device.states[A]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0
    );
    device.close();

    // (2) Its verdict asks a recovery.
    let device = Device::start();
    device.states[A].monitor_mode.store(1, Ordering::Release);
    let holder_claim = device.claim(B, ClaimKind::SelfCheck);
    let token = holder_claim
        .granted
        .clone()
        .unwrap_or_else(|| panic!("B's claim holds the free device: {holder_claim:?}"));
    device.configure_monitor(A, true);
    assert!(device.probe(A), "the probe is due");
    assert_eq!(
        device.probe_records(A),
        0,
        "no recovery coordination recorded"
    );
    assert!(device.host.fatal_error().expect("runtime health").is_none());
    device.release(&holder_claim, &token);
    assert!(
        device.probe(A),
        "the next round runs once the device is free"
    );
    let admitted = all_events(&device.host)
        .into_iter()
        .filter(|event| {
            event.event_type() == EventType::MonitorRecoveryAdmitted
                && event.links().instance_id() == Some(&device.ids[A])
        })
        .collect::<Vec<_>>();
    let [admitted] = admitted.as_slice() else {
        panic!("one recovery admitted: {admitted:#?}");
    };
    let EventPayload::Monitor(MonitorPayload::RecoveryAdmitted(detail)) = admitted.payload() else {
        panic!("a recovery admission: {admitted:?}");
    };
    assert_eq!(detail.recovery(), MonitorRecoveryKind::WakeStandby);
    assert_eq!(
        detail.reason(),
        MonitorRecoveryCoordinationReason::SchedulerAvailable
    );
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true), (A.to_owned(), false)]
    );
    device.close();
}

/// How B's own routine gate holds its queued claim back, in the test of a member's failed probe
/// capture beside a held member's queued claim.
#[derive(Debug, Clone, Copy)]
enum MemberHold {
    /// A scheduling pause of B.
    Paused,
    /// A #403 resource fence of B, which waits for a person (stuck recovery off).
    Fenced,
    /// A blackout of B's own list that covers now.
    Blackout,
}

/// Fix round 1 of MAPP-1d (the coordinator's ruling on the review's finding): a dedicated close
/// lease is refused, as before the slice, by every entry waiting in the device's queue that the
/// requesting member's own routine gate lets through (`grant_dedicated_instance_lease`, #411 A52).
/// B is held by its own gate (paused, fenced, inside a blackout of its own list) and its routine
/// startup claim waits in the device's queue, held; the device is free. A's probe runs on the free
/// device and its capture fails; the capture failure's dedicated close lease is refused by B's
/// claim, which A's open gate lets through (`lease_transfer_not_safe`), as for A alone with a
/// refusing queue: the refusal is recorded once (`runtime.failed`, naming A), no close is attempted
/// under it, A is not fenced, and the probe fails as a capture failure (`capture_failed` in A's
/// monitor state); the Runtime's health stays clean. Then, as the slice builds it, the probe's own
/// session close ahead closes A's session (fenced, confirmed) before the probe lets the device go,
/// and B's claim still waits in its place. Judged by each member's gate instead, B's held claim
/// refused nothing, the scheduler refused the close lease as a queue ahead (`lease_busy`), which
/// is no refusal by the queue, and the Runtime ended.
#[test]
fn mapp1d_a_members_failed_probe_capture_beside_a_held_members_claim_keeps_the_runtime_running() {
    for hold in [MemberHold::Paused, MemberHold::Fenced, MemberHold::Blackout] {
        let device = Device::start_with_blackout_of(match hold {
            MemberHold::Blackout => Some(B),
            MemberHold::Paused | MemberHold::Fenced => None,
        });
        match hold {
            MemberHold::Paused => device.pause(B),
            MemberHold::Fenced => device
                .host
                .fence_instance_for_test(B)
                .expect("B fenced by its own work's failure"),
            MemberHold::Blackout => {}
        }
        let held = device.claim(B, ClaimKind::StartupPackage);
        assert!(
            held.queued.is_some() && held.granted.is_none(),
            "{hold:?}: B's routine claim waits, held back by B's own gate: {held:?}"
        );
        assert_eq!(
            device
                .host
                .instance_claims_for_test(B)
                .expect("the device's claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "{hold:?}: the device is free and B's claim waits"
        );
        device.states[A].fail_capture.store(true, Ordering::Release);
        device.sample_capacity();
        device.configure_monitor(A, false);
        let mark = device.head();
        let probed = device.host.run_due_monitor_probe_for_test(A);
        assert!(
            matches!(probed, Ok(true)),
            "{hold:?}: A's probe ran on the free device and ended without a fatal: {probed:?}"
        );
        assert!(
            device.host.fatal_error().expect("runtime health").is_none(),
            "{hold:?}: the Runtime keeps running"
        );
        assert!(
            device.states[A].capture_count.load(Ordering::Acquire) > 0,
            "{hold:?}: A's capture was asked"
        );
        let after = device.events_after(mark);
        let refusals = after
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Runtime(RuntimePayload::Failed(record))
                    if record.outcome().code().as_str()
                        == SchedulerCode::LeaseTransferNotSafe.as_str() =>
                {
                    Some(record)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let [refusal] = refusals.as_slice() else {
            panic!(
                "{hold:?}: one recorded refusal of the close lease by the queue, found {}",
                refusals.len()
            );
        };
        assert_eq!(
            refusal
                .lifecycle_failure()
                .and_then(|failure| failure.instance_id()),
            Some(device.ids[A]),
            "{hold:?}: the refusal names A"
        );
        assert!(
            status_fence(&device.host, A).is_none(),
            "{hold:?}: no close was attempted under the refused lease, so A is not fenced"
        );
        let state = device.monitor_state(A).expect("A's monitor state");
        assert_eq!(
            state.last_error(),
            Some(RuntimeErrorCode::CaptureFailed),
            "{hold:?}: {state:?}"
        );
        let log = device.sessions();
        assert!(
            !log.is_empty()
                && log.iter().all(|(member, _)| member == A)
                && open_backends(&log, A) == 0,
            "{hold:?}: A's session, opened by its probe, closed by the probe's session close \
             ahead, and nothing of B opened: {log:?}"
        );
        assert_eq!(
            device.states[A]
                .unfenced_capture_close_count
                .load(Ordering::Acquire),
            0,
            "{hold:?}: A's capture closed under a fenced resource-close step"
        );
        assert_eq!(
            device
                .host
                .instance_claims_for_test(B)
                .expect("the device's claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "{hold:?}: B's claim still waits in its place, held"
        );
        device.close();
    }
}

/// What A's direct run's end work did and saw: B's claim, asked at it; the sweep's tick that
/// expired A's lapsed lease, whether it failed; the device's session log right after it; and the
/// Runtime clock then.
struct Lapse {
    claim: HostClaimForTest,
    swept: RuntimeHostResult<()>,
    at_sweep: Vec<(String, bool)>,
    granted_at: u64,
}

/// A direct run of A whose end work (its end phase entered, its lease renewed by the end grace)
/// asks B's claim, which waits for the device, then lets A's lease lapse past the end grace and
/// runs one tick of the lease sweep, after setting A's capture close unconfirmed when
/// `unconfirmed`. Returns the run's receipt and what its end work saw.
fn run_and_lapse(device: &Device, unconfirmed: bool) -> (RuntimeReceipt, Lapse) {
    let package = device.run_package(A);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            A,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    );
    let (sent, lapsed) = mpsc::channel();
    let host = Arc::clone(&device.host);
    let clock = Arc::clone(&device.clock);
    let a_state = Arc::clone(&device.states[A]);
    let log = Arc::clone(&device.log);
    let checkpoint = device
        .host
        .run_at_request_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            request.request_id(),
            device.ids[A],
            move |_| {
                let claim = host
                    .request_host_claim_for_test(B, ClaimKind::SelfCheck)
                    .expect("B's claim");
                if unconfirmed {
                    *a_state
                        .capture_close_error
                        .lock()
                        .expect("capture close error") = Some(unconfirmed_close());
                }
                clock.advance(LAPSE_MS);
                let swept = host.expire_due_leases_for_test();
                let at_sweep = log.lock().expect("the device's session log").clone();
                sent.send(Lapse {
                    claim,
                    swept,
                    at_sweep,
                    granted_at: clock.monotonic_ms(),
                })
                .expect("the lapse");
            },
        )
        .expect("arm A's end work");
    let receipt = device
        .host
        .process_request_for_test(&request, device.connection())
        .expect("A's run's receipt");
    assert_eq!(checkpoint.consumed(), 1, "A's run reached its end work");
    let lapse = lapsed.recv_timeout(GATE_WAIT).expect("the lapse");
    (receipt, lapse)
}

/// The hand-over at a lapsed run lease's expiry. A's direct run reaches its end work, B's claim
/// waits for the device, and A's lease lapses past its end grace; one tick of the lease sweep
/// expires it. The close of A's session under the lapsed lease is refused and fences A, as before
/// (#411 A64). On three hosts started alike:
/// - the slice off (before it): the expiry hands the device straight to B's claim (a transfer)
///   while A's session is still open;
/// - on: the expiry hands the device to no one, A's session is closed under a session close ahead
///   (fenced, confirmed) before B's lease begins, and B's claim, first in the queue, is granted by
///   the queue's own grant with B's member, B's holder and its own lease time; A stays fenced, the
///   run's failure is not fatal and the Runtime's health stays clean, and at no step are two
///   members' sessions open;
/// - on, with A's capture close unconfirmed: the close fences A (as an unconfirmed close does
///   today) and keeps its close lease, so the device is held for A and B's claim waits; the
///   Runtime's health stays clean.
#[test]
fn mapp1d_a_lapsed_run_lease_hands_the_device_on_only_after_the_members_session_is_closed() {
    for (off, unconfirmed) in [(true, false), (false, false), (false, true)] {
        let device = Device::start();
        device
            .host
            .mapp1d_off_for_test(off)
            .expect("the slice's branches");
        let (receipt, lapse) = run_and_lapse(&device, unconfirmed);
        assert!(lapse.swept.is_ok(), "the sweep's tick: {:?}", lapse.swept);
        assert!(
            receipt
                .error_projection()
                .is_none_or(|projection| !projection.fatal),
            "{receipt:?}"
        );
        assert!(
            device.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime keeps running"
        );
        assert!(
            status_fence(&device.host, A).is_some(),
            "the lapsed lease's refused close fenced A"
        );
        assert_eq!(
            lapse.claim.queued.as_ref().map(QueuedLease::position),
            Some(1),
            "B's claim waited first for the device: {:?}",
            lapse.claim
        );
        if off {
            assert!(
                open_backends(&lapse.at_sweep, A) > 0,
                "off: A's session is still open after the expiry: {:?}",
                lapse.at_sweep
            );
            let token = lapse
                .claim
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("off: the expiry handed the device to B's claim");
            let types = device
                .records_of(lapse.claim.request.request_id())
                .iter()
                .map(PersistedEvent::event_type)
                .collect::<Vec<_>>();
            assert!(
                types.contains(&EventType::LeaseTransferred),
                "off: by a transfer: {types:?}"
            );
            device.release(&lapse.claim, &token);
            device.close();
        } else if unconfirmed {
            assert!(
                lapse
                    .claim
                    .wait_for_grant(Duration::ZERO)
                    .expect("the grant slot")
                    .is_none(),
                "B's claim waits while A's unconfirmed close keeps its lease"
            );
            assert_eq!(
                device
                    .host
                    .instance_claims_for_test(B)
                    .expect("the device's claims"),
                (Some(ClaimKind::ResourceClose), vec![ClaimKind::SelfCheck]),
                "the device is held for A by its close lease, and B's claim waits in its place"
            );
            assert_eq!(
                device.states[A]
                    .unfenced_capture_close_count
                    .load(Ordering::Acquire),
                0
            );
            let Ok(host) = Arc::try_unwrap(device.host) else {
                panic!("a hook still holds the host");
            };
            // The Host close keeps an unconfirmed close's owner retained
            // (`read-session-resource-close.md`): the fence's own path, not this test's subject.
            let _closed = host.close();
        } else {
            assert!(
                open_backends(&lapse.at_sweep, A) == 0
                    && lapse.at_sweep.iter().any(|(member, _)| member == A),
                "on: A's session, opened by its run, is closed when the expiry ends: {:?}",
                lapse.at_sweep
            );
            let token = lapse
                .claim
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("on: B's claim is granted once A's session is closed");
            assert_eq!(token.instance_id(), device.ids[B]);
            assert_eq!(token.holder_id(), holder(&lapse.claim));
            assert_eq!(
                token.expires_at_monotonic_ms(),
                lapse.granted_at + CLAIM_TTL_MS,
                "the claim's own lease time from its grant"
            );
            let events = all_events(&device.host);
            assert!(
                first_close(&events, device.ids[A]) < grant_of(&events, &lapse.claim),
                "A's close is recorded before B's lease.granted"
            );
            assert_granted_from_the_queue(&device, &lapse.claim);
            let log = device.sessions();
            assert!(open_backends(&log, A) == 0 && !log.is_empty(), "{log:?}");
            assert_eq!(
                device.states[A]
                    .unfenced_capture_close_count
                    .load(Ordering::Acquire),
                0,
                "A's capture closed under a fenced resource-close step"
            );
            device.release(&lapse.claim, &token);
            device.close();
        }
    }
}

/// The queue's grant judges each claim by the gate of the member it serves. A is paused; during a
/// keyless observation of B on the free device A's startup claim and then B's self-check claim
/// queue (each meets the device's admission guard the observation holds); the observation's guard
/// pumps the queue when it lets the device go. On two hosts started alike, with the test-only
/// switch off and on. Off (one member's gate for the whole queue): A's claim is granted while A is
/// paused, ahead of B's. On: A's claim stays queued in its place and B's later claim is granted
/// by the queue's own grant; once A's pause is lifted and B's lease ends, A's claim is granted in
/// its place, by the queue's own grant.
#[test]
fn mapp1d_a_paused_members_claim_stays_queued_while_another_members_later_claim_is_granted() {
    for off in [true, false] {
        let device = Device::start();
        device
            .host
            .mapp1d_off_for_test(off)
            .expect("the slice's branches");
        device.pause(A);
        let (observed, (paused, later)) = device.observe_while(B, || {
            (
                device.claim(A, ClaimKind::StartupPackage),
                device.claim(B, ClaimKind::SelfCheck),
            )
        });
        assert_eq!(
            observed.state(),
            RuntimeReceiptState::Completed,
            "{observed:?}"
        );
        let position = |claim: &HostClaimForTest| claim.queued.as_ref().map(QueuedLease::position);
        assert_eq!((position(&paused), position(&later)), (Some(1), Some(2)));
        if off {
            let token = paused
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("off: A's claim is granted while A is paused");
            assert_eq!(token.instance_id(), device.ids[A]);
            assert!(
                later
                    .wait_for_grant(Duration::ZERO)
                    .expect("the grant slot")
                    .is_none()
            );
            device.release(&paused, &token);
            let token = later
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("B's claim after A's");
            device.release(&later, &token);
        } else {
            let token = later
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("on: B's later claim is granted");
            assert_eq!(token.instance_id(), device.ids[B]);
            assert_eq!(token.holder_id(), holder(&later));
            assert!(
                paused
                    .wait_for_grant(Duration::ZERO)
                    .expect("the grant slot")
                    .is_none(),
                "on: A's claim stays queued while A is paused"
            );
            assert_eq!(
                device
                    .host
                    .instance_claims_for_test(A)
                    .expect("the device's claims"),
                (Some(ClaimKind::SelfCheck), vec![ClaimKind::StartupPackage]),
                "on: B holds the device and A's claim waits in its place"
            );
            assert_granted_from_the_queue(&device, &later);
            device
                .host
                .lift_instance_pause_for_test(A)
                .expect("A's pause lifted");
            device.release(&later, &token);
            let token = paused
                .wait_for_grant(GATE_WAIT)
                .expect("the grant slot")
                .expect("on: A's claim is granted in its place once its pause is lifted");
            assert_eq!(token.instance_id(), device.ids[A]);
            assert_eq!(token.holder_id(), holder(&paused));
            assert_granted_from_the_queue(&device, &paused);
            device.release(&paused, &token);
        }
        device.close();
    }
}

/// The test-only switch against main's code, on one host, two rounds: the switch on, then off.
/// Off, the queue's grant (`pump`) names the member whose queue was pumped in its candidate's
/// performance check, its ladder judgement and its `lease.granted`, and every claim of the queue
/// is judged by that member's gate, as on main; on, it names the member the candidate serves (read
/// with it, `first_eligible_claim`) and judges each claim by its own member's gate. A first probe
/// round before the compared ones takes whatever a first probe of an instance takes once.
/// - A member of several, where the switch makes a difference: during A's keyless observation on
///   the free device B's self-check claim queues, and the observation's guard pumps A's queue when
///   it lets the device go, which grants B's claim from the queue; the lease sweep's scan is held
///   meanwhile, so no other thread can grant it (the comment at that step). The `lease.granted`
///   record of B's claim names B (the claim's own member) on, and A (the member whose queue was
///   pumped) off.
/// - An instance alone (C), where it must make none: C's probe, the expiry of C's lease while C's
///   claim waits, and the queue's grant of C's startup claim queued during C's observation (a kind
///   that takes the pump's performance check) run main's code off and the slice's on, and write
///   the same records: type, severity, the instance the links name and whole payload, record by
///   record, with every id, time, registry place and evaluation `seed` normalised
///   ([`comparable`]), every difference named in one failure. What must not happen is asserted
///   directly too: C's probe takes no lease and keeps C's session (opened by C's first
///   observation, closed only by the Host close); C's lapsed lease hands the key to its waiting
///   claim by a transfer; C's queued claim is granted by the queue's own grant. Neither C part
///   depends on which thread acts: the expiry of C's lease is the test's sweep tick or the
///   resident sweep's, one after the other under the scan's gate and the same records either way,
///   and every pump that can grant C's claim is a pump of C's own queue, which names C.
///
/// The probe's and the expiry's group branches begin from a member of a device group of several,
/// and C reaches none of them. The slice's refactorings that C runs (`probe_monitor_instance` out
/// of `run_monitor_probe`, `end_lease_ordered` out of `cleanup_token_guarded`, the admission
/// guard handed through `cleanup_token_inner`, `device_held_by_another_member` out of the keyless
/// observation's refusal) are not under the switch: this test does not claim them (register
/// §1.359).
#[test]
fn mapp1d_the_switch_changes_a_members_grant_record_and_no_record_of_a_lone_instance() {
    let device = Device::start();
    let c = Arc::clone(&device.states[C]);
    let session = || {
        (
            c.capture_open_count.load(Ordering::Acquire),
            c.capture_close_count.load(Ordering::Acquire),
        )
    };
    device.sample_capacity();
    let opened = device.observe(C);
    assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");
    assert_eq!(session(), (1, 0), "C's first observation opens its session");
    device.configure_monitor(C, false);
    assert!(device.probe(C), "the first probe round");
    device.clear_monitor(C);
    let connection = device.connection();
    let mut rounds = Vec::new();
    let mut member_grants = Vec::new();
    for off in [false, true] {
        device
            .host
            .mapp1d_off_for_test(off)
            .expect("the slice's branches");

        // C's probe.
        device.sample_capacity();
        device.configure_monitor(C, false);
        let grants = device.grants_to(C);
        let mark = device.head();
        assert!(device.probe(C), "C's probe is due");
        assert_eq!(device.grants_to(C), grants, "C's probe takes no lease");
        assert_eq!(session(), (1, 0), "C keeps its session");
        let correlation = device
            .events_after(mark)
            .iter()
            .find(|event| event.event_type() == EventType::MonitorProbeRequested)
            .and_then(|event| event.links().correlation_id().copied())
            .expect("the probe's request");
        let probe = device.comparable_records(EventQuery {
            correlation_id: Some(correlation),
            ..EventQuery::default()
        });
        device.clear_monitor(C);

        // C's lease lapses while C's claim waits; one sweep tick expires it.
        let acquire = device.request(RuntimeOperation::acquire_lease(
            C,
            IdentifierIssuer::new()
                .expect("issuer")
                .mint_holder_id()
                .expect("holder"),
        ));
        let granted = device
            .host
            .process_request_for_test(&acquire, connection)
            .expect("C's lease");
        assert!(
            matches!(granted.result(), Some(RuntimeResult::LeaseGranted { .. })),
            "C's lease: {granted:?}"
        );
        let waiting = device.claim(C, ClaimKind::SelfCheck);
        assert_eq!(
            waiting.queued.as_ref().map(QueuedLease::position),
            Some(1),
            "{waiting:?}"
        );
        let mark = device.head();
        device.clock.advance(LAPSE_MS);
        device
            .host
            .expire_due_leases_for_test()
            .expect("one sweep tick");
        let handed = waiting
            .wait_for_grant(GATE_WAIT)
            .expect("the grant slot")
            .expect("C's claim is handed the key at the expiry");
        let expired = device
            .events_after(mark)
            .into_iter()
            .filter(|event| {
                event.links().instance_id() == Some(&device.ids[C])
                    && event.payload().artifact_retention().is_none()
            })
            .collect::<Vec<_>>();
        assert!(
            expired
                .iter()
                .any(|event| event.event_type() == EventType::LeaseTransferred),
            "C's lapsed lease is handed on by a transfer: {expired:#?}"
        );
        let expiry = comparable(&expired);
        device.release(&waiting, &handed);

        // C's startup claim queued during C's observation, granted by the queue's own grant.
        device.sample_capacity();
        let (observed, queued) =
            device.observe_while(C, || device.claim(C, ClaimKind::StartupPackage));
        assert_eq!(
            queued.queued.as_ref().map(QueuedLease::position),
            Some(1),
            "{queued:?}"
        );
        assert_eq!(
            observed.state(),
            RuntimeReceiptState::Completed,
            "{observed:?}"
        );
        let pumped = queued
            .wait_for_grant(GATE_WAIT)
            .expect("the grant slot")
            .expect("C's claim is granted once the observation lets the device go");
        assert_granted_from_the_queue(&device, &queued);
        let grant = device.comparable_records(EventQuery {
            request_id: Some(queued.request.request_id()),
            ..EventQuery::default()
        });
        device.release(&queued, &pumped);
        assert_eq!(session(), (1, 0), "C keeps its session");
        rounds.push((probe, expiry, grant));

        // A member of several: B's claim queued during A's observation, granted by the pump of
        // A's queue when A's observation lets the device go. Only that pump can grant it, so the
        // record cannot depend on timing: the test holds the lease sweep's scan
        // (`hold_lease_expiry_scan_for_test`), so the sweep, the one thread that pumps every
        // device's queue on its own (`pump_all`, at the end of that scan), waits at the scan's
        // start until the claim is granted; the resident monitor loop is held; B's claim's own
        // pump (`request_host_claim`) runs while A's observation holds the device's admission
        // guard and finds it taken; no lease holds the device, so no lease end hands it on; and
        // this host has no idle action configured and is driven through no policy cycle. The pump
        // of A's queue runs on the observing thread when the observation's guard drops, before
        // its receipt returns, so the claim is granted when `observe_while` returns (a zero wait).
        device.sample_capacity();
        let scan = device
            .host
            .hold_lease_expiry_scan_for_test()
            .expect("hold the lease sweep's scan");
        let (observed_a, member_claim) =
            device.observe_while(A, || device.claim(B, ClaimKind::SelfCheck));
        assert_eq!(
            observed_a.state(),
            RuntimeReceiptState::Completed,
            "{observed_a:?}"
        );
        assert_eq!(
            member_claim.queued.as_ref().map(QueuedLease::position),
            Some(1),
            "B's claim waited for the device: {member_claim:?}"
        );
        let member_token = member_claim
            .wait_for_grant(Duration::ZERO)
            .expect("the grant slot")
            .expect("B's claim granted by the pump of A's queue before observe returned");
        drop(scan);
        assert_eq!(member_token.instance_id(), device.ids[B]);
        assert_granted_from_the_queue(&device, &member_claim);
        let named = device
            .records_of(member_claim.request.request_id())
            .iter()
            .find(|event| event.event_type() == EventType::LeaseGranted)
            .and_then(|event| event.links().instance_id().copied());
        device.release(&member_claim, &member_token);
        member_grants.push(named);
    }
    device
        .host
        .mapp1d_off_for_test(false)
        .expect("the slice's branches");
    assert_eq!(
        member_grants,
        vec![Some(device.ids[B]), Some(device.ids[A])],
        "the lease.granted of B's claim names B, its own member, with the switch on, and A, the \
         member whose queue was pumped, with it off"
    );
    let [on, off] = rounds.as_slice() else {
        panic!("two rounds: {rounds:#?}");
    };
    let mut found = Vec::new();
    for (part, with, without) in [
        ("the probe", &on.0, &off.0),
        ("the expiry", &on.1, &off.1),
        ("the queue's grant", &on.2, &off.2),
    ] {
        assert!(!with.is_empty(), "{part} has records");
        found.extend(differences(part, with, without));
    }
    assert!(
        found.is_empty(),
        "the rounds differ in {} places:\n{}",
        found.len(),
        found.join("\n")
    );
    let log = device.sessions();
    assert!(
        log.iter().all(|(member, _)| member == A) && open_backends(&log, A) == 0,
        "the group's device saw A's observations only, each session closed: {log:?}"
    );
    device.close();
    assert_eq!(
        session(),
        (1, 1),
        "C's session is closed by the Host close, as before"
    );
}
