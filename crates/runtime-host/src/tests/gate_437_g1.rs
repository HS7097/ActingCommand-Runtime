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
use super::instance_workers::all_events;
use super::*;
use crate::codes::HostCode;
use crate::host::{ContainedTaskCheckpointPoint, HostClaimForTest};
use actingcommand_contract::{HolderId, InstanceStuckRecovery};
use actingcommand_policy::DeviceGroupWait;
use actingcommand_scheduler::{ClaimKind, QueuedLease};
use serde_json::json;

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

/// A's direct run whose end work asks B's self-check claim (which waits for the device), sets
/// A's capture close unconfirmed and lets A's lease lapse past its end grace, and one tick of the
/// lease sweep, as MAPP-1d's lapse does: the close of A's session under the lapsed lease is
/// refused and fences A, and the session close ahead that follows is unconfirmed, so A's close
/// lease is kept. Then the clock moves past that close lease's time and one more tick ends it.
/// Returns B's claim, which waited first.
fn fence_a_and_lapse_its_close_lease(device: &Device) -> HostClaimForTest {
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
                *a_state
                    .capture_close_error
                    .lock()
                    .expect("capture close error") = Some(unconfirmed_close());
                clock.advance(LAPSE_MS);
                let swept = host.expire_due_leases_for_test();
                sent.send((claim, swept)).expect("the lapse");
            },
        )
        .expect("arm A's end work");
    let receipt = device
        .host
        .process_request_for_test(&request, device.connection())
        .expect("A's run's receipt");
    assert_eq!(checkpoint.consumed(), 1, "A's run reached its end work");
    let (claim, swept) = waited.recv_timeout(GATE_WAIT).expect("the lapse");
    assert!(swept.is_ok(), "the sweep's tick: {swept:?}");
    assert!(
        receipt
            .error_projection()
            .is_none_or(|projection| !projection.fatal),
        "{receipt:?}"
    );
    assert!(
        status_fence(&device.host, A).is_some(),
        "the lapsed lease's refused close fenced A"
    );
    assert_eq!(
        claim.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "B's claim waited first for the device: {claim:?}"
    );
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
/// fenced and the Runtime's health is clean.
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
