// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 G1 (register §1.433; `contracts/device-group.md`, "A failure stays where it
//! belongs", "Device-level actions", row 7, and "One queue, one worker, one lease per device"):
//! the two items the contract's "Release." listed before the load may accept a group of several.
//!
//! - **Rule 1.** While a member is fenced (its close is not confirmed), the device is given to no
//!   other member: the end of its kept close lease names no transfer, the queue's grant passes the
//!   other members' claims over in their places, their requests for the device are refused naming
//!   the fenced member, and their keyless observation and monitor probe meet the device as another
//!   member's lease. The fenced member's own way out is unchanged; once its fence is lifted the
//!   claims that waited are granted in their places.
//! - **Rule 2.** A claim its own member's performance directive or ladder wait holds stays in its
//!   place and does not hold back another member's claim behind it.
//!
//! Two fixture instances A and B are one device group of several (`device.one`) and C is alone on
//! its own device. The load refuses every group of several, so the host is handed the member →
//! device key map and the group's name as the assembly builds them. A and B write one fake
//! device's session log and close their captures only under a fenced resource-close step. A manual
//! Runtime clock whose monotonic time moves only when a test (or a run's own wait) moves it; stuck
//! recovery off, so a fence waits for a person and no ladder acts; the resident monitor loop held.
//! A member's performance suspension and a ladder that starts after performance release are the
//! test-only stand-ins `suspend_member_performance_for_test` sets. Every expected value is the
//! contract's or the coordinator's ruling; every comparison is within one host. Neutral names only.

use super::gate_403::status_fence;
use super::instance_workers::{all_events, phases, wait_until};
use super::*;
use crate::codes::HostCode;
use crate::host::{ContainedTaskCheckpointPoint, HostClaimForTest};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    DeviceHeldWait, HolderId, InstanceDeviceHeld, InstanceStuckRecovery, RecoveryTriggerStage,
    RuntimeLifecyclePhase, SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_policy::{DeviceGroupWait, PolicyCode};
use actingcommand_scheduler::{ClaimKind, QueuedLease};
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
const GROUP: &str = "device.one";
/// Bounds a broken run's wait only; nothing sleeps.
const GATE_WAIT: Duration = Duration::from_secs(120);
/// The monitor's interval: the longest the policy takes, so a probe that did not run stays due.
const MONITOR_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
/// How far A's run's end work moves the Runtime clock to let the run's lease lapse: past its
/// response deadline, the heartbeat reserve and the run's end grace.
const LAPSE_MS: u64 = 60 * 60 * 1_000;
/// How far a test then moves the clock to let A's kept close lease lapse: past the scheduler's
/// lease time of the fixture (5 000 ms), well inside the first wait of A's fence for its recheck
/// (300 000 ms), so no recheck of A runs meanwhile.
const CLOSE_LAPSE_MS: u64 = 10_000;

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
            next_connection: AtomicU64::new(3_000),
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

    /// `operation` processed as a client's on `connection`.
    fn process_on(&self, operation: RuntimeOperation, connection: ConnectionId) -> RuntimeReceipt {
        self.host
            .process_request_for_test(&self.request(operation), connection)
            .expect("the request's receipt")
    }

    /// A client's `AcquireLease` of `alias` on `connection`.
    fn acquire_on(&self, alias: &str, connection: ConnectionId) -> RuntimeReceipt {
        self.process_on(
            RuntimeOperation::acquire_lease(
                alias,
                IdentifierIssuer::new()
                    .expect("issuer")
                    .mint_holder_id()
                    .expect("holder"),
            ),
            connection,
        )
    }

    /// The client's release of `token` on `connection`, the connection that holds it.
    fn release_on(&self, token: &LeaseToken, connection: ConnectionId) {
        let released = self.process_on(
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
            connection,
        );
        assert_eq!(
            released.state(),
            RuntimeReceiptState::Completed,
            "{released:?}"
        );
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

    /// The device's holder and its waiting claims, in the queue's order.
    fn claims(&self, alias: &str) -> (Option<ClaimKind>, Vec<ClaimKind>) {
        self.host
            .instance_claims_for_test(alias)
            .expect("the device's claims")
    }

    /// A scheduling pause of `alias`, as a client asks for it (MAPP-1d's): its routine claims wait.
    fn pause(&self, alias: &str) {
        let mut client = TestClient::connect(&self.host);
        let pause = client.request(RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: alias.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("g1_member_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        });
        let receipt = client.send(&pause);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// A keyless read-only observation of `alias`.
    fn observe(&self, alias: &str) -> RuntimeReceipt {
        self.process(RuntimeOperation::ObserveReadonly {
            instance_alias: alias.to_owned(),
        })
    }

    /// A keyless observation of `alias` whose capture waits in the fake while `during` runs, so
    /// that `during` acts while the observation holds the device's admission guard. Its guard's
    /// drop pumps the device's queue.
    fn observe_while<T>(&self, alias: &'static str, during: impl FnOnce() -> T) -> T {
        let (entered_tx, entered) = mpsc::channel::<()>();
        let (proceed_tx, proceed) = mpsc::channel::<()>();
        *self.states[alias]
            .capture_hook
            .lock()
            .expect("capture hook") = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = proceed.recv_timeout(GATE_WAIT);
        }));
        let (observed, during) = thread::scope(|scope| {
            let observer = scope.spawn(|| self.observe(alias));
            entered
                .recv_timeout(GATE_WAIT)
                .expect("the observation's capture is in flight");
            let during = during();
            proceed_tx.send(()).expect("the capture goes on");
            (observer.join().expect("the observer"), during)
        });
        assert_eq!(
            observed.state(),
            RuntimeReceiptState::Completed,
            "{observed:?}"
        );
        during
    }

    /// `alias`'s monitor configured: one probe per `MONITOR_INTERVAL_MS` of the page `home`, no
    /// recovery. It is due at once.
    fn configure_monitor(&self, alias: &str) {
        let receipt = self.process(RuntimeOperation::ConfigureMonitor {
            instance_alias: alias.to_owned(),
            policy: RuntimeMonitorPolicy::new(MONITOR_INTERVAL_MS, "home", false)
                .expect("monitor policy"),
        });
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// `alias`'s monitor state, as the monitor's status reads it.
    fn monitor_state(&self, alias: &str) -> Option<actingcommand_contract::RuntimeMonitorState> {
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

    /// Every record of the request `request_id`, in ledger order.
    fn types_of(&self, request_id: actingcommand_contract::RequestId) -> Vec<EventType> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            })
            .expect("the request's records")
            .iter()
            .map(PersistedEvent::event_type)
            .collect()
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

    /// The device's session log so far; at every step of it at most one member has a session
    /// open.
    fn assert_one_session_at_a_time(&self) {
        let entries = self.log.lock().expect("the device's session log").clone();
        let mut open = BTreeMap::<String, usize>::new();
        for (step, (member, opened)) in entries.iter().enumerate() {
            let count = open.entry(member.clone()).or_default();
            *count = if *opened {
                *count + 1
            } else {
                count.saturating_sub(1)
            };
            assert!(
                open.values().filter(|count| **count > 0).count() <= 1,
                "step {step}: two members' sessions open on the device at once: {entries:?}"
            );
        }
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

    fn assert_runs(&self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime keeps running"
        );
    }

    /// The Host close of a host on which an unconfirmed close was met: it keeps that close's owner
    /// retained (`read-session-resource-close.md`), the fence's own path, not these tests' subject.
    fn close_unchecked(self) {
        let Ok(host) = Arc::try_unwrap(self.host) else {
            panic!("a hook still holds the host");
        };
        let _closed = host.close();
    }

    fn close(self) {
        self.assert_runs();
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

/// A capture close that ends without confirming that its connection is gone.
fn unconfirmed_close() -> DeviceError {
    DeviceError::fatal("synthetic capture close left its connection unconfirmed")
        .with_resource_summary(
            actingcommand_device::DeviceResourceQuiescence::Unconfirmed,
            1,
        )
}

/// `receipt` is the refusal `device_held_for_member_close` naming the fenced member `fenced`.
fn assert_held_for(receipt: &RuntimeReceipt, fenced: &str) {
    assert!(
        matches!(
            receipt.state(),
            RuntimeReceiptState::Denied | RuntimeReceiptState::Failed
        ),
        "{receipt:?}"
    );
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        HostCode::DeviceHeldForMemberClose.as_str(),
        "{receipt:?}"
    );
    assert_eq!(outcome.value("alias"), Some(&json!(fenced)), "{receipt:?}");
}

/// What A's run of [`run_a_with_b_waiting`] left: B's claim, which waited first; the kind of the
/// device's holder right after its end work's sweep tick; A's run's receipt and its request.
struct RunEnd {
    claim: HostClaimForTest,
    holder_after_sweep: Option<ClaimKind>,
    receipt: RuntimeReceipt,
    request_id: actingcommand_contract::RequestId,
}

/// A's direct run whose end work asks B's self-check claim (which waits for the device), sets
/// A's capture close unconfirmed when `unconfirmed`, and, when `lapse`, lets A's lease lapse past
/// its end grace and runs one tick of the lease sweep, as MAPP-1d's lapse does: the close of A's
/// session under the lapsed lease is refused and fences A (#411 A64, a refused close), and the
/// session close ahead that follows closes A's session, confirmed or not. Without `lapse` the run
/// ends on its own lease, whose end closes A's session under it. The end work reads the device's
/// holder after its sweep tick.
fn run_a_with_b_waiting(device: &Device, unconfirmed: bool, lapse: bool) -> RunEnd {
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
    let (sent, waited) = mpsc::channel();
    let host = Arc::clone(&device.host);
    let clock = Arc::clone(&device.clock);
    let a_state = Arc::clone(&device.states[A]);
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
                let swept = if lapse {
                    clock.advance(LAPSE_MS);
                    host.expire_due_leases_for_test()
                } else {
                    Ok(())
                };
                let holder = host.instance_claims_for_test(A).map(|(holder, _)| holder);
                sent.send((claim, swept, holder)).expect("the end work");
            },
        )
        .expect("arm A's end work");
    let receipt = device
        .host
        .process_request_for_test(&request, device.connection())
        .expect("A's run's receipt");
    assert_eq!(checkpoint.consumed(), 1, "A's run reached its end work");
    let (claim, swept, holder) = waited.recv_timeout(GATE_WAIT).expect("the end work");
    assert!(swept.is_ok(), "the sweep's tick: {swept:?}");
    let holder_after_sweep = holder.expect("the device's holder after the sweep's tick");
    assert!(
        receipt
            .error_projection()
            .is_none_or(|projection| !projection.fatal),
        "{receipt:?}"
    );
    assert!(
        status_fence(&device.host, A).is_some(),
        "A's refused or unconfirmed close fenced A"
    );
    assert_eq!(
        claim.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "B's claim waited first for the device: {claim:?}"
    );
    RunEnd {
        claim,
        holder_after_sweep,
        receipt,
        request_id: request.request_id(),
    }
}

/// [`run_a_with_b_waiting`] with A's capture close unconfirmed and A's lease lapsed: the session
/// close ahead that follows the refused close is unconfirmed, so A's close lease is kept. Then the
/// clock moves past that close lease's time and one more tick ends it. Returns B's claim, which
/// waited first.
fn fence_a_and_lapse_its_close_lease(device: &Device) -> HostClaimForTest {
    let claim = run_a_with_b_waiting(device, true, true).claim;
    assert_eq!(
        device.claims(B),
        (Some(ClaimKind::ResourceClose), vec![ClaimKind::SelfCheck]),
        "A's unconfirmed close keeps its close lease and B's claim waits"
    );
    device.clock.advance(CLOSE_LAPSE_MS);
    device
        .host
        .expire_due_leases_for_test()
        .expect("the sweep's tick that ends A's kept close lease");
    claim
}

// ---- Rule 1.

/// A's close is unconfirmed, A is fenced and its kept close lease lapses. The end of that lease
/// hands the device to no one: no lease holds it, B's claim is not granted and waits first, in its
/// place, and nothing of B was granted. No run is dispatched to B: a dispatch of B is refused with
/// `device_group_waiting` `close_unconfirmed` naming A (the policy admission's hold of B), and B's
/// direct run is refused `device_held_for_member_close`, naming A, and nothing of it ran. A stays
/// fenced and the Runtime's health is clean. Fix round 1: the refused close of the lapsed lease
/// (at the expiry and at the run's end) holds nothing, so only the unconfirmed session close ahead
/// can hold the device here: without its mark the end of the close lease would hand the device to
/// B's claim and this test would fail.
#[test]
fn g1_a_members_unconfirmed_close_keeps_the_device_from_the_other_member_after_its_close_lease() {
    let device = Device::start();
    let waiting = fence_a_and_lapse_its_close_lease(&device);
    device.assert_runs();
    assert!(status_fence(&device.host, A).is_some(), "A stays fenced");
    assert_eq!(
        device
            .host
            .device_group_wait_for_test(B)
            .expect("B's hold as the admission reads it"),
        Some((DeviceGroupWait::CloseUnconfirmed, A.to_owned())),
        "B's dispatch waits for A's unconfirmed close"
    );
    assert_eq!(
        device
            .host
            .device_group_wait_for_test(A)
            .expect("A's hold as the admission reads it"),
        None,
        "A's own fence keeps its own code"
    );
    assert_eq!(
        device.claims(B),
        (None, vec![ClaimKind::SelfCheck]),
        "no lease holds the device and B's claim waits in its place"
    );
    assert!(
        waiting
            .wait_for_grant(Duration::ZERO)
            .expect("the grant slot")
            .is_none(),
        "B's claim is not granted"
    );
    assert_eq!(device.grants_to(B), 0, "nothing of B was granted");
    let package = device.run_package(B);
    let run = device.process(RuntimeOperation::run_contained_task(
        B,
        IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder"),
        ContainedTaskRequest::new(package).expect("task request"),
    ));
    assert_held_for(&run, A);
    assert_eq!(device.grants_to(B), 0, "B's run took no lease");
    assert_eq!(
        device.states[B].capture_count.load(Ordering::Acquire),
        0,
        "nothing of B ran on the device"
    );
    assert_eq!(device.claims(B), (None, vec![ClaimKind::SelfCheck]));
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.close_unchecked();
}

/// Then A's fence is lifted by A's own way out, a person's self-check of A that passes (A's
/// capture closes again): A's self-check is granted the free device (B's waiting claim is passed
/// over while A is fenced), its passing self-check lifts the fence, and its key's end hands the
/// device to B's claim, which waited first, in its place: granted under its own request with B's
/// member and holder, never queued again.
#[test]
fn g1_once_the_fence_is_lifted_the_claim_that_waited_is_granted_in_its_place() {
    let device = Device::start();
    let waiting = fence_a_and_lapse_its_close_lease(&device);
    *device.states[A]
        .capture_close_error
        .lock()
        .expect("capture close error") = None;
    // The fake's opens report a passed native open, so A's self-check passes (gate_403).
    device.states[A]
        .observed_open
        .store(true, Ordering::Release);
    let checked = device.process(RuntimeOperation::SelfCheckInstance {
        instance_alias: A.to_owned(),
    });
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    assert!(
        status_fence(&device.host, A).is_none(),
        "A's passing self-check lifted its fence"
    );
    let token = waiting
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted once A's fence is lifted");
    assert_eq!(token.instance_id(), device.ids[B]);
    assert_eq!(token.holder_id(), holder(&waiting));
    let types = device.types_of(waiting.request.request_id());
    assert_eq!(
        types
            .iter()
            .filter(|event_type| **event_type == EventType::SchedulerQueued)
            .count(),
        1,
        "B's claim was queued once and never again: {types:?}"
    );
    assert!(
        types.contains(&EventType::LeaseGranted) || types.contains(&EventType::LeaseTransferred),
        "B's claim was handed the device under its own request: {types:?}"
    );
    assert_eq!(device.claims(B), (Some(ClaimKind::SelfCheck), vec![]));
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.release(&waiting, &token);
    device.close_unchecked();
}

/// While A is fenced for an unconfirmed close, a keyless observation of B and B's monitor probe
/// meet the device as they meet another member's lease: the observation is refused
/// `device_group_action_refused` (B, `observe_readonly`) and captures nothing; the probe's round
/// is not made, twice (no capture, no lease, B's monitor state unchanged and still due).
#[test]
fn g1_another_members_observation_and_probe_meet_the_fenced_device_as_another_members_lease() {
    let device = Device::start();
    device
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    let observed = device.observe(B);
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Denied,
        "{observed:?}"
    );
    let outcome = observed.outcome();
    assert_eq!(
        outcome.code().as_str(),
        HostCode::DeviceGroupActionRefused.as_str(),
        "{observed:?}"
    );
    assert_eq!(outcome.value("alias"), Some(&json!(B)), "{observed:?}");
    assert_eq!(
        outcome.value("operation"),
        Some(&json!("observe_readonly")),
        "{observed:?}"
    );
    device.configure_monitor(B);
    let before = device.monitor_state(B).expect("B's monitor state");
    for round in 0..2 {
        assert!(
            device
                .host
                .run_due_monitor_probe_for_test(B)
                .expect("a probe round"),
            "round {round}: the probe is still due"
        );
    }
    assert_eq!(
        device.states[B].capture_count.load(Ordering::Acquire),
        0,
        "nothing of B captured"
    );
    assert_eq!(device.grants_to(B), 0, "no lease of B");
    assert_eq!(
        device.monitor_state(B),
        Some(before),
        "B's monitor state is unchanged: no run counted, no verdict replaced, still due"
    );
    device.assert_runs();
    device.close_unchecked();
}

/// While A is fenced for an unconfirmed close, a client's lease of B is refused
/// `device_held_for_member_close`, naming A, and nothing of B is granted; B's queued lease waits
/// in its place. A's own way out is unchanged: a person's lease of A is granted the free device,
/// and its end hands the device to no one.
#[test]
fn g1_a_manual_operation_on_the_other_member_is_refused_naming_the_fenced_member() {
    let device = Device::start();
    device
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    assert_held_for(&device.acquire_on(B, device.connection()), A);
    assert_eq!(device.grants_to(B), 0, "nothing of B was granted");
    let queued = device.process(RuntimeOperation::queue_lease(
        B,
        IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder"),
        LeaseQueuePolicy::new(LeasePriority::Normal, 30_000).expect("queue policy"),
    ));
    assert_eq!(queued.state(), RuntimeReceiptState::Queued, "{queued:?}");
    assert_eq!(device.claims(B), (None, vec![ClaimKind::ClientLease]));
    let own_connection = device.connection();
    let own = device.acquire_on(A, own_connection);
    let Some(RuntimeResult::LeaseGranted { token }) = own.result() else {
        panic!("a person's lease of the fenced member passes its fence: {own:?}");
    };
    assert_eq!(token.instance_id(), device.ids[A]);
    device.release_on(token, own_connection);
    assert_eq!(
        device.claims(B),
        (None, vec![ClaimKind::ClientLease]),
        "the end of A's lease hands the device to no one while A is fenced"
    );
    assert_eq!(device.grants_to(B), 0);
    device.assert_runs();
    device.close_unchecked();
}

/// An instance alone on its device whose close is unconfirmed, as before: a person's lease of it
/// is granted (no refusal of the new kind), and the end of that lease hands its key to its own
/// claim that waited meanwhile.
#[test]
fn g1_an_instance_alone_with_its_close_unconfirmed_is_as_before() {
    let device = Device::start();
    device
        .host
        .fence_instance_unconfirmed_for_test(C)
        .expect("fence C for an unconfirmed close");
    let connection = device.connection();
    let lease = device.acquire_on(C, connection);
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a person's lease of a fenced instance alone passes its fence: {lease:?}");
    };
    let waiting = device.claim(C, ClaimKind::SelfCheck);
    assert_eq!(
        waiting.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "{waiting:?}"
    );
    device.release_on(token, connection);
    let granted = waiting
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("C's own claim is handed C's key at the lease's end");
    assert_eq!(granted.instance_id(), device.ids[C]);
    device.release(&waiting, &granted);
    device.assert_runs();
    device.close_unchecked();
}

/// A failure of the Runtime's own on the new path (the read of whether a member's device is held
/// for another member's unconfirmed close, failing as a poisoned lock does) still ends the whole
/// Runtime: its health holds `runtime_state_poisoned`.
#[test]
fn g1_a_failure_of_the_runtimes_own_on_the_device_hold_is_still_fatal() {
    let device = Device::start();
    device
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    device
        .host
        .fail_next_device_hold_read_for_test()
        .expect("arm the failure");
    let _receipt = device.host.process_request_for_test(
        &device.request(RuntimeOperation::acquire_lease(
            B,
            IdentifierIssuer::new()
                .expect("issuer")
                .mint_holder_id()
                .expect("holder"),
        )),
        device.connection(),
    );
    let fatal = device
        .host
        .fatal_error()
        .expect("runtime health")
        .expect("the Runtime's own failure is fatal");
    assert_eq!(fatal.code(), HostCode::RuntimeStatePoisoned.as_str());
    device.close_unchecked();
}

// ---- Rule 1, fix round 1.

/// The device hold `Status` reports in `alias`'s row.
fn status_device_held(device: &Device, alias: &str) -> Option<InstanceDeviceHeld> {
    let receipt = device.process(RuntimeOperation::Status);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status result: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == alias)
        .expect("the instance's status row")
        .device_held()
        .cloned()
}

/// How many start preparations of `alias` ran (`instance_preparation_finished` at stage
/// `startup_preparation` under the instance), and whether the last one passed.
fn start_preparations(device: &Device, alias: &str) -> (usize, bool) {
    let events = all_events(&device.host);
    let finished = phases(&events)
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&device.ids[alias]))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::InstancePreparationFinished {
                stage: RecoveryTriggerStage::StartupPreparation,
                outcome,
                ..
            } => Some(outcome.is_none()),
            _ => None,
        })
        .collect::<Vec<_>>();
    (finished.len(), finished.last().copied().unwrap_or(false))
}

/// How many `runtime.failed` records under `alias` carry `device_held_for_member_close` (a
/// preparation of it that the device's hold refused).
fn held_failures(device: &Device, alias: &str) -> usize {
    all_events(&device.host)
        .iter()
        .filter(|event| {
            event.event_type() == EventType::RuntimeFailed
                && event.links().instance_id() == Some(&device.ids[alias])
                && event.payload().outcome().is_some_and(|outcome| {
                    outcome.code().as_str() == HostCode::DeviceHeldForMemberClose.as_str()
                })
        })
        .count()
}

/// The train's fix round 1: `alias`'s last start preparation was recorded under the request of
/// the claim whose key it ran on, a request that asked for the device and was granted it
/// (`lease.requested`, `lease.granted`): the device's worker made it on the claim's key
/// (`run_held_start_preparation`, the claim's links). Fix round 1's preparation on the lease
/// sweep's own thread was recorded under links the sweep minted, on a dedicated lease granted under
/// a request of its own, so its request holds neither record.
fn assert_prepared_under_its_claim(device: &Device, alias: &str) {
    let events = all_events(&device.host);
    let request = phases(&events)
        .into_iter()
        .rev()
        .find(|(event, phase)| {
            event.links().instance_id() == Some(&device.ids[alias])
                && matches!(
                    phase,
                    RuntimeLifecyclePhase::InstancePreparationFinished {
                        stage: RecoveryTriggerStage::StartupPreparation,
                        ..
                    }
                )
        })
        .and_then(|(event, _)| event.links().request_id().copied())
        .unwrap_or_else(|| panic!("the request of {alias}'s start preparation"));
    let types = device.types_of(request);
    assert!(
        types.contains(&EventType::LeaseRequested) && types.contains(&EventType::LeaseGranted),
        "{alias}'s start preparation was recorded under its claim's request: {types:?}"
    );
}

/// The review's must-fix 1: MAPP-1d's lapsed run lease whose session close ahead is confirmed. A's
/// run's lease lapses in its end phase while B's claim waits; the close under the lapsed lease is
/// refused and fences A (#411 A64), at the expiry and again at the run's end, and the session close
/// ahead closes A's session, confirmed. Fix round 2: the run's end really met the lapsed lease:
/// right after the end work's sweep tick no lease of A's run holds the device, so the run's end
/// found its lease gone (#411 A64's settlement at the run's end), and A's run does not complete
/// (its own lease end met the lapsed lease too). The train's fix round 1: that failure is the
/// lease's expiry, not any failure: the lease A's run was granted has one `lease.expired` record
/// (the sweep's tick), and the run's receipt is its own lease end's refusal of a lease the sweep
/// had ended (`lease_mismatch`, the scheduler's answer to a token no longer the device's: its
/// release validates it, ISO-5a row B17). A refused close holds nothing: B's claim is granted once
/// A's session is closed, B's dispatch waits only for A's fence (`fenced`, not
/// `close_unconfirmed`) and B's status row names no hold; once B's lease ends, a new lease of B is
/// granted the device.
#[test]
fn g1_a_refused_close_whose_session_close_ahead_is_confirmed_holds_the_device_for_no_one() {
    let device = Device::start();
    let run = run_a_with_b_waiting(&device, false, true);
    assert_ne!(
        run.holder_after_sweep,
        Some(ClaimKind::DirectTaskRun),
        "the sweep's tick ended A's run's lapsed lease before the run's end"
    );
    assert_ne!(
        run.receipt.state(),
        RuntimeReceiptState::Completed,
        "A's run's end met its lapsed lease: {:?}",
        run.receipt
    );
    assert_eq!(
        run.receipt.outcome().code().as_str(),
        RuntimeErrorCode::LeaseMismatch.as_str(),
        "A's run's own lease end met the lease the sweep had ended: {:?}",
        run.receipt
    );
    let events = all_events(&device.host);
    let run_lease = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::LeaseGranted
                && event.links().request_id() == Some(&run.request_id)
        })
        .and_then(|event| event.links().lease_id().copied())
        .expect("the lease A's run was granted");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseExpired
                    && event.links().lease_id() == Some(&run_lease)
            })
            .count(),
        1,
        "the sweep's tick expired the lease of A's run, once"
    );
    let waiting = run.claim;
    let token = waiting
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted once A's session is closed");
    assert_eq!(token.instance_id(), device.ids[B]);
    device.release(&waiting, &token);
    assert!(status_fence(&device.host, A).is_some(), "A stays fenced");
    assert_eq!(
        device
            .host
            .device_group_wait_for_test(B)
            .expect("B's hold as the admission reads it"),
        Some((DeviceGroupWait::Fenced, A.to_owned())),
        "B's dispatch waits for A's fence, which holds no device"
    );
    assert_eq!(status_device_held(&device, B), None, "no hold in B's row");
    let connection = device.connection();
    let lease = device.acquire_on(B, connection);
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a new lease of B is granted the device: {lease:?}");
    };
    assert_eq!(token.instance_id(), device.ids[B]);
    device.release_on(token, connection);
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.close_unchecked();
}

/// The review's (i): an ordinary lease's end whose close is unconfirmed, not a dedicated close
/// lease. A's direct run ends on its own lease while B's claim waits; the close of A's session at
/// that lease's end is unconfirmed and fences A. That end hands the device to no one: no lease
/// holds it, B's claim waits first in its place and nothing of B was granted; a client's lease of B
/// is refused `device_held_for_member_close` naming A; and B's status row says the device is held
/// (`close_unconfirmed`, A), while A's and C's rows name no hold.
#[test]
fn g1_an_ordinary_lease_end_whose_close_is_unconfirmed_hands_the_device_to_no_one() {
    let device = Device::start();
    let waiting = run_a_with_b_waiting(&device, true, false).claim;
    assert_eq!(
        device.claims(B),
        (None, vec![ClaimKind::SelfCheck]),
        "the end of A's run's lease hands the device to no one and B's claim waits in its place"
    );
    assert!(
        waiting
            .wait_for_grant(Duration::ZERO)
            .expect("the grant slot")
            .is_none(),
        "B's claim is not granted"
    );
    assert_eq!(device.grants_to(B), 0, "nothing of B was granted");
    assert_held_for(&device.acquire_on(B, device.connection()), A);
    assert_eq!(device.grants_to(B), 0, "B's lease was not granted");
    assert_eq!(
        status_device_held(&device, B),
        Some(InstanceDeviceHeld {
            device_group_wait: DeviceHeldWait::CloseUnconfirmed,
            alias: A.to_owned(),
        }),
        "B's status row says the device is held for A's unconfirmed close"
    );
    assert_eq!(
        status_device_held(&device, A),
        None,
        "A's row keeps its fence"
    );
    assert_eq!(status_device_held(&device, C), None, "C is alone");
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.close_unchecked();
}

/// Item 3 of fix round 1, as fix round 2 builds it: a member whose start preparation the hold
/// refused comes back by itself, and the lease sweep only queues that preparation. While A is
/// fenced for an unconfirmed close, B's start preparation (the daemon start's) is refused
/// `device_held_for_member_close`, which leaves B unavailable: that failure is recorded once, and
/// the sweep's step, run twice while A's fence stands, records no other, queues nothing for B and
/// prepares nothing. Once A's own self-check passes and lifts A's fence, the step queues B's
/// preparation on a self-check claim of the Runtime's own: B's worker makes it on that claim's key,
/// once, it passes and the key is released; the step then queues nothing more for B. B's lease is
/// then granted the device. The train's fix round 1: B's preparation is recorded under that claim's
/// request (the worker's links), which fix round 1's preparation on the sweep's own thread was not.
#[test]
fn g1_a_member_whose_start_preparation_the_hold_refused_is_prepared_again_once_the_fence_is_lifted()
{
    let device = Device::start();
    // The fake's opens report a passed native open, so a self-check passes (gate_403).
    for member in [A, B] {
        device.states[member]
            .observed_open
            .store(true, Ordering::Release);
    }
    device
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    let refused = device
        .host
        .prepare_on_start_for_test(B)
        .expect("B's start preparation");
    assert_eq!(
        refused.as_deref(),
        Some(HostCode::DeviceHeldForMemberClose.as_str()),
        "B's start preparation is refused while A's fence holds the device"
    );
    for tick in 0..2 {
        device
            .host
            .queue_members_after_device_hold_for_test()
            .expect("the sweep's step");
        assert_eq!(
            held_failures(&device, B),
            1,
            "tick {tick}: B's refused preparation is recorded once"
        );
        assert_eq!(
            device.claims(B),
            (None, vec![]),
            "tick {tick}: nothing is queued for B while A's fence stands"
        );
        assert_eq!(
            start_preparations(&device, B),
            (0, false),
            "tick {tick}: B is not prepared while A's fence stands"
        );
    }
    let checked = device.process(RuntimeOperation::SelfCheckInstance {
        instance_alias: A.to_owned(),
    });
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    assert!(
        status_fence(&device.host, A).is_none(),
        "A's passing self-check lifted its fence"
    );
    device
        .host
        .queue_members_after_device_hold_for_test()
        .expect("the sweep's step");
    wait_until("B's start preparation on its claim's key", || {
        start_preparations(&device, B).0 == 1
    });
    wait_until("the key of B's preparation released", || {
        device.claims(B) == (None, vec![])
    });
    assert_eq!(
        start_preparations(&device, B),
        (1, true),
        "B's start preparation ran again, once, and passed"
    );
    assert_prepared_under_its_claim(&device, B);
    device
        .host
        .queue_members_after_device_hold_for_test()
        .expect("the sweep's step");
    assert_eq!(
        device.claims(B),
        (None, vec![]),
        "nothing more is queued for B"
    );
    assert_eq!(start_preparations(&device, B), (1, true));
    assert_eq!(held_failures(&device, B), 1);
    let connection = device.connection();
    let lease = device.acquire_on(B, connection);
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("B's lease is granted the device: {lease:?}");
    };
    device.release_on(token, connection);
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.close_unchecked();
}

/// The train's fix round 1, item 2: the sweep's step queues a held start preparation once the
/// device is idle by the scheduler's own judgement of an eligible entry, so another member's claim
/// parked behind its own pause does not keep it back. A is paused and A's startup claim waits in
/// the device's queue, held back by A's own gate. A is then fenced for an unconfirmed close, which
/// refuses B's start preparation (recorded once), and A's own self-check passes and lifts A's
/// fence; A's claim still waits, parked behind A's pause. The step queues B's preparation on a
/// claim of the Runtime's own: B's worker makes it once, under that claim's request, it passes and
/// the key is released; the step then queues nothing more for B, and A's claim still waits in its
/// place, never granted. Judged by the device's queue being empty, as before, A's parked claim
/// would keep B's preparation from ever being queued.
#[test]
fn g1_another_members_claim_parked_behind_its_own_pause_leaves_the_held_preparation_queued() {
    let device = Device::start();
    // The fake's opens report a passed native open, so a self-check passes (gate_403).
    for member in [A, B] {
        device.states[member]
            .observed_open
            .store(true, Ordering::Release);
    }
    device.pause(A);
    let parked = device.claim(A, ClaimKind::StartupPackage);
    assert!(
        parked.queued.is_some() && parked.granted.is_none(),
        "A's startup claim waits, held back by A's own pause: {parked:?}"
    );
    device
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    let refused = device
        .host
        .prepare_on_start_for_test(B)
        .expect("B's start preparation");
    assert_eq!(
        refused.as_deref(),
        Some(HostCode::DeviceHeldForMemberClose.as_str()),
        "B's start preparation is refused while A's fence holds the device"
    );
    let checked = device.process(RuntimeOperation::SelfCheckInstance {
        instance_alias: A.to_owned(),
    });
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    assert!(
        status_fence(&device.host, A).is_none(),
        "A's passing self-check lifted its fence"
    );
    device
        .host
        .queue_members_after_device_hold_for_test()
        .expect("the sweep's step");
    wait_until("B's start preparation on its claim's key", || {
        start_preparations(&device, B).0 == 1
    });
    wait_until("the key of B's preparation released", || {
        device.claims(B) == (None, vec![ClaimKind::StartupPackage])
    });
    assert_eq!(
        start_preparations(&device, B),
        (1, true),
        "B's start preparation ran, once, and passed"
    );
    assert_prepared_under_its_claim(&device, B);
    device
        .host
        .queue_members_after_device_hold_for_test()
        .expect("the sweep's step");
    assert_eq!(
        device.claims(B),
        (None, vec![ClaimKind::StartupPackage]),
        "nothing more is queued for B, and A's claim waits in its place"
    );
    assert_eq!(start_preparations(&device, B), (1, true));
    assert_eq!(held_failures(&device, B), 1);
    assert!(
        parked
            .wait_for_grant(Duration::ZERO)
            .expect("the grant slot")
            .is_none(),
        "A's claim is not granted while A is paused"
    );
    device.assert_one_session_at_a_time();
    device.assert_runs();
    device.close_unchecked();
}

// ---- A real policy cycle (fix round 1).

const TASK_A: &str = "fixture.member-a";
const TASK_B: &str = "fixture.member-b";
const SEED: u64 = 11;

/// One task of `instance`, due at every evaluation (a local interval clock of 1 ms), from the
/// fixture catalog's generic task, as gate_mapp1b's.
fn member_task(base: &Value, id: &str, instance: &str, priority: u32) -> Value {
    let mut spec = base.clone();
    spec["id"] = json!(id);
    spec["scope"] = json!({"kind": "instance", "instance_id": instance});
    spec["priority"] = json!(priority);
    spec["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                     "every_ms": 1, "anchor_ms": 0}
    });
    spec["cooldown_ms"] = json!(0);
    spec["instance_overrides"] = json!([]);
    spec["produces"] = json!([]);
    spec["on_failure"]["escalation_threshold"] = json!(1000);
    spec["loop_budget"] = json!({
        "daily_limit": 48,
        "window_iteration_limit": 48,
        "max_runtime_ms": 300_000
    });
    spec
}

/// The catalog: TASK_A on A and TASK_B on B, under the fixture's game-wide activity profile with
/// no interval between dispatches, as gate_mapp1b's.
fn member_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = json!([
        member_task(&base, TASK_A, A, 300),
        member_task(&base, TASK_B, B, 200),
    ]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let mut profile = activity["profiles"][1].clone();
    profile["daily_budget"] = json!(48);
    profile["max_window_iterations"] = json!(48);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    activity["profiles"] = json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    sources
}

/// A host whose instances A and B are the device group `GROUP` with the catalog of
/// [`member_sources`] active, on a manual Runtime clock at the fixture's noon, stuck recovery off.
struct Policy {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    _root: TempDir,
}

impl Policy {
    fn start() -> Self {
        let root = TempDir::new().expect("tempdir");
        let a = instance_id();
        let b = instance_id();
        let device = a.min(b);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        facts.instances.push(InstanceSnapshot {
            instance_id: B.to_owned(),
            server_id: "fixture-server-b".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-b".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
        let resources = pending_policy_resources();
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let stuck_off = |alias: &str| {
            (
                alias.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )
        };
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), resources.clone()))
                .with_runtime_clock(clock.clone())
                .with_stuck_recovery(BTreeMap::from([stuck_off(A), stuck_off(B)]))
                .with_device_keys(BTreeMap::from([(a, device), (b, device)]))
                .with_device_group_names(BTreeMap::from([
                    (A.to_owned(), GROUP.to_owned()),
                    (B.to_owned(), GROUP.to_owned()),
                ])),
            Arc::new(FakeProvider::from_entries([
                (A.to_owned(), a, Arc::new(FakeState::default())),
                (B.to_owned(), b, Arc::new(FakeState::default())),
            ])),
        )
        .expect("start the device group host");
        host.activate_policy_catalog(&member_sources())
            .expect("activate the catalog");
        Self {
            host,
            clock,
            facts,
            resources,
            _root: root,
        }
    }

    /// One evaluation at `unix_ms`, the capacity sampled there first. The Runtime clock's
    /// monotonic time does not move.
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }
}

/// The review's (i): "no run is dispatched to the other member" through a real policy cycle, not
/// a helper's read. A and B each have a task due at every evaluation, and A is fenced for an
/// unconfirmed close. The cycle dispatches nothing to B: no dispatch intent names B, and the last
/// reason of B's task is `device_group_waiting` with `device_group_wait` `close_unconfirmed`,
/// naming A.
#[test]
fn g1_a_policy_cycle_dispatches_nothing_to_the_other_member_while_the_device_is_held() {
    let policy = Policy::start();
    policy
        .host
        .fence_instance_unconfirmed_for_test(A)
        .expect("fence A for an unconfirmed close");
    let cycle = policy.evaluate(POLICY_NOW_UNIX_MS);
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    assert!(
        !evaluation
            .dispatch_intents
            .iter()
            .any(|intent| intent.instance_id == B),
        "{:#?}",
        evaluation.dispatch_intents
    );
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == TASK_B && decision.instance_id.as_deref() == Some(B))
        .unwrap_or_else(|| panic!("a decision of {TASK_B} on {B}: {evaluation:#?}"));
    let outcome: &Outcome = &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason of {TASK_B}: {decision:?}"))
        .outcome;
    assert!(
        outcome.is(PolicyCode::DeviceGroupWaiting),
        "device_group_waiting: {outcome:?}"
    );
    assert_eq!(outcome.value("instance_id"), Some(&json!(B)), "{outcome:?}");
    assert_eq!(outcome.value("alias"), Some(&json!(A)), "{outcome:?}");
    assert_eq!(
        outcome.value("device_group_wait"),
        Some(&json!(DeviceGroupWait::CloseUnconfirmed.as_str())),
        "{outcome:?}"
    );
    let Policy { host, .. } = policy;
    let _closed = host.close();
}

// ---- The host train with #475 (G1 fix round 2's item 3, built without a test).

/// A person's lease of A on `connection`, which holds the device.
fn a_persons_lease(device: &Device, connection: ConnectionId) -> LeaseToken {
    let lease = device.acquire_on(A, connection);
    let Some(RuntimeResult::LeaseGranted { token }) = lease.result() else {
        panic!("a person's lease of A holds the device: {lease:?}");
    };
    token.clone()
}

/// A person's self-check of B (an operator claim, whose bound is the product's own wait for a
/// self-check) asked while A's lease holds the device: it waits in the device's queue, and once it
/// waits `meanwhile` runs. Returns B's receipt, which its bound ends.
fn b_self_check_waits_out_its_bound(device: &Device, meanwhile: impl FnOnce()) -> RuntimeReceipt {
    thread::scope(|scope| {
        let checking = scope.spawn(|| {
            device.process(RuntimeOperation::SelfCheckInstance {
                instance_alias: B.to_owned(),
            })
        });
        wait_until("B's self-check waits for the device", || {
            device.claims(B).1 == vec![ClaimKind::SelfCheck]
        });
        meanwhile();
        checking.join().expect("B's self-check")
    })
}

/// The `caused_by` links of `outcome`.
fn caused_by(outcome: &Outcome) -> Vec<&Outcome> {
    outcome
        .causes()
        .iter()
        .filter(|link| link.relation().as_str() == "caused_by")
        .map(|link| link.outcome())
        .collect()
}

/// An operator's claim that waits out its bound while the device is held for a fenced member
/// names the hold, and only then. The train's fix round 1 adds the opposite case first, on a host
/// of its own: a person's lease of A holds the device, no member is fenced, and a person's
/// self-check of B waits for the device until its bound: it is refused `lease_queue_expired` with
/// A's lease as its holder (`holder_kind` `client_lease`) and no `caused_by` link of
/// `device_held_for_member_close`, and it was never granted. Then the hold: a person's lease of A
/// holds the device and B's self-check waits for it in the device's queue; A is then fenced for an
/// unconfirmed close, which holds the device for A, and A's lease is released, whose end hands the
/// device to no one: from then on the hold is the only thing that keeps B waiting (an operator's
/// claim asked while the hold stands is refused at once, so B must be queued behind A's lease
/// first). At its bound B's self-check is refused `lease_queue_expired` with no holder of the
/// device (no `holder_kind`), whose one `caused_by` link is `device_held_for_member_close` naming
/// A; it was never granted, and its entry has left the queue.
#[test]
fn g1_an_operator_claim_that_waits_out_its_bound_while_the_device_is_held_names_the_fenced_member()
{
    let device = Device::start();
    let connection = device.connection();
    let token = a_persons_lease(&device, connection);
    let checked = b_self_check_waits_out_its_bound(&device, || {});
    assert_eq!(checked.state(), RuntimeReceiptState::Denied, "{checked:?}");
    let outcome = checked.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "lease_queue_expired",
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("holder_kind"),
        Some(&json!("client_lease")),
        "A's lease held the device when B's bound ended: {outcome:?}"
    );
    let hold_code = HostCode::DeviceHeldForMemberClose.as_str();
    assert!(
        caused_by(outcome)
            .iter()
            .all(|cause| cause.code().as_str() != hold_code),
        "no hold, so no cause of the hold: {outcome:?}"
    );
    assert_eq!(device.grants_to(B), 0, "B's self-check was never granted");
    device.release_on(&token, connection);
    device.close();

    let device = Device::start();
    let connection = device.connection();
    let token = a_persons_lease(&device, connection);
    let checked = b_self_check_waits_out_its_bound(&device, || {
        device
            .host
            .fence_instance_unconfirmed_for_test(A)
            .expect("fence A for an unconfirmed close");
        device.release_on(&token, connection);
    });
    assert_eq!(checked.state(), RuntimeReceiptState::Denied, "{checked:?}");
    let outcome = checked.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "lease_queue_expired",
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("holder_kind"),
        None,
        "no lease held the device when B's bound ended: only the hold kept B waiting: {outcome:?}"
    );
    let causes = caused_by(outcome);
    let [held] = causes.as_slice() else {
        panic!("one caused_by link: {outcome:?}");
    };
    assert_eq!(
        held.code().as_str(),
        HostCode::DeviceHeldForMemberClose.as_str(),
        "{outcome:?}"
    );
    assert_eq!(held.value("alias"), Some(&json!(A)), "{outcome:?}");
    assert_eq!(device.grants_to(B), 0, "B's self-check was never granted");
    assert!(
        device.claims(B).1.is_empty(),
        "B's entry left the queue at its bound"
    );
    device.assert_runs();
    device.close_unchecked();
}

// ---- Rule 2.

/// A member's own wait on two members: during an observation of B on the free device, A's claim
/// and then B's queue; `held_back` holds A's claim (A's performance suspension, or its ladder that
/// starts after performance release). The observation's guard pumps the device's queue: B's claim
/// is granted by the queue's own grant while A's stays first in its place; once A is released and
/// B's lease ends, A's claim is granted next.
fn a_members_own_wait_leaves_the_queue_to_the_other(kind: ClaimKind, ladder_after_release: bool) {
    let device = Device::start();
    device
        .host
        .suspend_member_performance_for_test(A, true, ladder_after_release)
        .expect("suspend A");
    let (first, later) = device.observe_while(B, || {
        (device.claim(A, kind), device.claim(B, ClaimKind::SelfCheck))
    });
    let position = |claim: &HostClaimForTest| claim.queued.as_ref().map(QueuedLease::position);
    assert_eq!((position(&first), position(&later)), (Some(1), Some(2)));
    let token = later
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim behind A's is granted");
    assert_eq!(token.instance_id(), device.ids[B]);
    assert_eq!(token.holder_id(), holder(&later));
    let types = device.types_of(later.request.request_id());
    assert!(
        types.contains(&EventType::LeaseGranted) && !types.contains(&EventType::LeaseTransferred),
        "by the queue's own grant: {types:?}"
    );
    assert!(
        first
            .wait_for_grant(Duration::ZERO)
            .expect("the grant slot")
            .is_none(),
        "A's claim waits"
    );
    assert_eq!(
        device.claims(A),
        (Some(ClaimKind::SelfCheck), vec![kind]),
        "B holds the device and A's claim stays first in its place"
    );
    device
        .host
        .suspend_member_performance_for_test(A, false, false)
        .expect("release A");
    device.release(&later, &token);
    let token = first
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("A's claim is granted next");
    assert_eq!(token.instance_id(), device.ids[A]);
    assert_eq!(token.holder_id(), holder(&first));
    device.release(&first, &token);
    device.close();
}

/// Rule 2 with A's performance directive: A's startup claim (a kind that takes today's admission
/// checks) is held by A's suspension.
#[test]
fn g1_a_members_claim_its_directive_withholds_stays_first_while_the_other_members_is_granted() {
    a_members_own_wait_leaves_the_queue_to_the_other(ClaimKind::StartupPackage, false);
}

/// Rule 2 with A's ladder: A's ladder claim that waits for performance release.
#[test]
fn g1_a_members_ladder_waiting_for_release_stays_first_while_the_other_members_is_granted() {
    a_members_own_wait_leaves_the_queue_to_the_other(ClaimKind::Ladder, true);
}

/// An instance alone on its device: a candidate its directive withholds still grants nothing, as
/// before, even with a claim behind it that its directive would let through; once C is released
/// the withheld claim is granted first.
#[test]
fn g1_an_instance_alones_withheld_candidate_still_grants_nothing() {
    let device = Device::start();
    device
        .host
        .suspend_member_performance_for_test(C, true, false)
        .expect("suspend C");
    let (first, later) = device.observe_while(C, || {
        (
            device.claim(C, ClaimKind::StartupPackage),
            device.claim(C, ClaimKind::SelfCheck),
        )
    });
    assert_eq!(
        device.claims(C),
        (None, vec![ClaimKind::StartupPackage, ClaimKind::SelfCheck]),
        "nothing is granted behind C's withheld claim"
    );
    for claim in [&first, &later] {
        assert!(
            claim
                .wait_for_grant(Duration::ZERO)
                .expect("the grant slot")
                .is_none()
        );
    }
    device
        .host
        .suspend_member_performance_for_test(C, false, false)
        .expect("release C");
    device
        .host
        .expire_due_leases_for_test()
        .expect("the sweep's tick pumps the queue");
    let token = first
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("C's withheld claim is granted first once C is released");
    assert_eq!(token.holder_id(), holder(&first));
    device.release(&first, &token);
    let token = later
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("C's later claim after it");
    device.release(&later, &token);
    device.close();
}
