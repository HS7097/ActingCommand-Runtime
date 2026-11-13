// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1c (register §1.350; `contracts/device-group.md`, "Device-level actions",
//! rows 7 and 15, and "The session close of a keyless observation"): the two cases in which
//! MAPP-1b left a member's session open after a keyless observation (register §1.330, point 11),
//! closed, through the host, and the pins of the lock walk.
//!
//! Two fixture instances A and B are one device group of several (`device.one`) and C is alone on
//! its own device. The load refuses every group of several (actingd's MAPP-1 gate), so the host is
//! handed the member → device key map and the group's name as the assembly builds them. A and B
//! write one fake device's session log: every capture or input backend each opens or closes, in
//! the order the device saw them; every capture close of theirs must run under a fenced
//! resource-close step. A manual Runtime clock whose monotonic time moves only when a test moves
//! it, so a lease taken stays unexpired and a takeover cooldown lasts until the test ends it;
//! nothing sleeps. Every expected value is the contract's or the coordinator's ruling; every
//! comparison is within one host. Neutral names only.

use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::HostCode;
use crate::host::HostClaimForTest;
use actingcommand_contract::{HolderId, ResourceQuiescence, RuntimeLifecyclePhase};
use actingcommand_scheduler::{ClaimKind, QueuedLease};
use serde_json::json;

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
const GROUP: &str = "device.one";
/// Bounds a broken run's wait only; nothing sleeps.
const GATE_WAIT: Duration = Duration::from_secs(120);
/// The lease time `request_host_claim_for_test` gives every claim it asks for.
const CLAIM_TTL_MS: u64 = 60_000;
/// The fixture scheduler's takeover cooldown (`config`'s `takeover_cooldown_ms`).
const TAKEOVER_COOLDOWN_MS: u64 = 40;

// ---- The fixture.

/// A host whose instances A and B are the device group `GROUP` and C is alone, on a manual
/// Runtime clock whose monotonic time starts at 0.
struct Device {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    log: DeviceSessionLog,
    next_connection: AtomicU64,
    _root: TempDir,
}

impl Device {
    /// With `cooldown`, the start gives the group's device the takeover cooldown, as after an
    /// unclean end that listed A.
    fn start(cooldown: bool) -> Self {
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
        let mut config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_device_keys(BTreeMap::from([(ids[A], device), (ids[B], device)]))
            .with_device_group_names(BTreeMap::from([
                (A.to_owned(), GROUP.to_owned()),
                (B.to_owned(), GROUP.to_owned()),
            ]));
        if cooldown {
            config = config.with_takeover_cooldown_for_test([ids[A]]);
        }
        let entries = states
            .iter()
            .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
            .collect::<Vec<_>>();
        let host = RuntimeHost::start(config, Arc::new(FakeProvider::from_entries(entries)))
            .expect("start the device group's host");
        Self {
            host,
            clock,
            ids,
            states,
            log,
            next_connection: AtomicU64::new(1_000),
            _root: root,
        }
    }

    fn connection(&self) -> ConnectionId {
        ConnectionId::new(self.next_connection.fetch_add(1, Ordering::AcqRel))
            .expect("a test connection")
    }

    fn request(&self, operation: RuntimeOperation) -> RuntimeRequest {
        runtime_request(&IdentifierIssuer::new().expect("issuer"), operation)
    }

    /// One keyless read-only observation of `alias`.
    fn observe(&self, alias: &str) -> (RuntimeRequest, RuntimeReceipt) {
        let request = self.request(RuntimeOperation::ObserveReadonly {
            instance_alias: alias.to_owned(),
        });
        let receipt = self
            .host
            .process_request_for_test(&request, self.connection())
            .expect("the observation's receipt");
        (request, receipt)
    }

    /// A keyless observation of `alias` whose capture waits in the fake while `during` runs, so
    /// `during` acts while the observation holds the device's admission guard with the member's
    /// capture open. Returns the observation's receipt and what `during` returned.
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
            let observer = scope.spawn(|| self.observe(alias).1);
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

    /// The device's session log so far; at every step of it at most one member has a session
    /// open, and no member closes more than it opened.
    fn sessions(&self) -> Vec<(String, bool)> {
        let entries = self.log.lock().expect("the device's session log").clone();
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

    /// The type and severity of every record of `query`, in ledger order, but a frame's retention
    /// records, which the frame cleaner writes on its own time.
    fn shapes(&self, query: EventQuery) -> Vec<(EventType, EventSeverity)> {
        self.host
            .query_persisted_events_for_test(query)
            .expect("records")
            .iter()
            .filter(|event| event.payload().artifact_retention().is_none())
            .map(|event| (event.event_type(), event.severity()))
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

    fn close(self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime keeps running"
        );
        self.host.close().expect("close the device group's host");
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

/// `receipt` is the denial `device_group_action_refused` of `operation` on the member `alias`.
fn assert_refused(receipt: &RuntimeReceipt, alias: &str, operation: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        HostCode::DeviceGroupActionRefused.as_str(),
        "{receipt:?}"
    );
    assert_eq!(outcome.value("alias"), Some(&json!(alias)), "{receipt:?}");
    assert_eq!(
        outcome.value("operation"),
        Some(&json!(operation)),
        "{receipt:?}"
    );
}

// ---- The tests.

/// Gap one: a keyless observation of A on a free device, during whose capture B's claim and then
/// A's own claim queue (each meets the device's admission guard the observation holds). A's
/// session is closed before the observation lets the device go, under a session close ahead that
/// the waiting claims do not refuse (no refusal is recorded); then B's claim, first in the queue,
/// is granted by the queue's own grant, after A's close, with B's member, B's holder and its own
/// lease time, while A's claim stays queued behind it. The device's log shows A's session closed
/// before B's opens, and at no step two members' sessions open; every capture close is fenced.
#[test]
fn mapp1c_a_members_keyless_session_is_closed_before_a_claim_queued_meanwhile_is_granted() {
    let device = Device::start(false);
    let (observed, (first, second)) = device.observe_while(A, || {
        assert_eq!(
            device.sessions(),
            vec![(A.to_owned(), true)],
            "A's capture is open"
        );
        (
            device.claim(B, ClaimKind::SelfCheck),
            device.claim(A, ClaimKind::SelfCheck),
        )
    });
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let queued = |claim: &HostClaimForTest| {
        claim
            .queued
            .as_ref()
            .map(QueuedLease::position)
            .unwrap_or_else(|| panic!("the claim waited for the device: {claim:?}"))
    };
    assert_eq!((queued(&first), queued(&second)), (1, 2));

    let token = first
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted once the observation lets the device go");
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true), (A.to_owned(), false)],
        "A's session is closed, and nothing of B's is open, when B's lease begins"
    );
    assert_eq!(token.instance_id(), device.ids[B]);
    assert_eq!(token.holder_id(), holder(&first));
    assert_eq!(
        token.expires_at_monotonic_ms(),
        device.clock.monotonic_ms() + CLAIM_TTL_MS,
        "the claim's own lease time from its grant"
    );
    assert_eq!(
        device
            .host
            .instance_claims_for_test(B)
            .expect("the device's claims"),
        (Some(ClaimKind::SelfCheck), vec![ClaimKind::SelfCheck]),
        "A's claim waits behind B's, in its place"
    );
    assert!(
        second
            .wait_for_grant(Duration::ZERO)
            .expect("slot")
            .is_none()
    );
    let events = all_events(&device.host);
    assert!(
        first_close(&events, device.ids[A]) < grant_of(&events, &first),
        "A's close is recorded before B's lease.granted"
    );
    assert!(
        !events.iter().any(|event| {
            event.event_type() == EventType::RuntimeFailed
                && event.links().instance_id() == Some(&device.ids[A])
        }),
        "no close lease of A was refused and nothing of A failed"
    );
    assert_granted_from_the_queue(&device, &first);

    // B's own session, under B's lease, opens after A's is closed and closes at B's lease end.
    let (_, observed_b) = device.observe(B);
    assert_eq!(
        observed_b.state(),
        RuntimeReceiptState::Completed,
        "{observed_b:?}"
    );
    device.release(&first, &token);
    let a_token = second
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("A's claim is granted after B's lease end");
    device.release(&second, &a_token);
    assert_eq!(
        device.sessions(),
        vec![
            (A.to_owned(), true),
            (A.to_owned(), false),
            (B.to_owned(), true),
            (B.to_owned(), false),
        ]
    );
    for member in [A, B] {
        assert_eq!(
            device.states[member]
                .unfenced_capture_close_count
                .load(Ordering::Acquire),
            0,
            "{member}'s captures closed under a fenced resource-close step"
        );
    }
    device.close();
}

/// Gap two, in the device's takeover cooldown: the same observation of A while B's claim queues
/// during its capture. The session close ahead is granted through the cooldown and closes A's
/// session before the observation ends (fenced, confirmed), and the observation completes; the
/// cooldown is neither ended nor shortened (a lease of B asked for meanwhile is refused
/// `lease_cooldown`) and B's claim stays queued, granted to no one, until the cooldown ends; then
/// the queue's own grant gives it the key, after A's close, with its own lease time. At no step
/// are two members' sessions open on the device.
#[test]
fn mapp1c_in_the_takeover_cooldown_the_session_is_closed_and_the_queued_claim_waits_for_its_end() {
    let device = Device::start(true);
    let (observed, first) = device.observe_while(A, || device.claim(B, ClaimKind::SelfCheck));
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    assert_eq!(
        first.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "{first:?}"
    );
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true), (A.to_owned(), false)],
        "A's session is closed in the cooldown"
    );
    assert_eq!(
        device.states[A]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0,
        "under a fenced resource-close step"
    );
    assert!(
        first
            .wait_for_grant(Duration::ZERO)
            .expect("slot")
            .is_none()
    );
    assert_eq!(
        device
            .host
            .instance_claims_for_test(B)
            .expect("the device's claims"),
        (None, vec![ClaimKind::SelfCheck]),
        "the device is free and B's claim waits"
    );
    let acquire = device.request(RuntimeOperation::acquire_lease(
        B,
        IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder"),
    ));
    let refused = device
        .host
        .process_request_for_test(&acquire, device.connection())
        .expect("the acquisition's receipt");
    assert_eq!(
        refused.error_projection().map(|error| error.code),
        Some(RuntimeErrorCode::LeaseCooldown),
        "the cooldown still runs: {refused:?}"
    );

    device.clock.advance(TAKEOVER_COOLDOWN_MS);
    device
        .host
        .expire_due_leases_for_test()
        .expect("one sweep tick");
    let token = first
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted once the cooldown ended");
    assert_eq!(token.instance_id(), device.ids[B]);
    assert_eq!(token.holder_id(), holder(&first));
    assert_eq!(
        token.expires_at_monotonic_ms(),
        device.clock.monotonic_ms() + CLAIM_TTL_MS
    );
    let events = all_events(&device.host);
    assert!(first_close(&events, device.ids[A]) < grant_of(&events, &first));
    assert_granted_from_the_queue(&device, &first);

    let (_, observed_b) = device.observe(B);
    assert_eq!(
        observed_b.state(),
        RuntimeReceiptState::Completed,
        "{observed_b:?}"
    );
    device.release(&first, &token);
    assert_eq!(
        device.sessions(),
        vec![
            (A.to_owned(), true),
            (A.to_owned(), false),
            (B.to_owned(), true),
            (B.to_owned(), false),
        ]
    );
    device.close();
}

/// No close runs while a run of any member holds the device. While B's run (its direct run's
/// lease) holds it, a keyless observation of A is refused before any device step: nothing opens
/// and no lease of A is taken. While A's own run holds it, A's observation is served and its
/// session stays open, with no close lease taken, until that run's lease end closes it; B's claim,
/// queued meanwhile, is granted only after that close.
#[test]
fn mapp1c_no_session_close_runs_while_a_run_of_any_member_holds_the_device() {
    let device = Device::start(false);
    let sample_capacity = device
        .host
        .capacity_sampler_for_test()
        .expect("capacity owner");
    sample_capacity().expect("capacity sample");

    let b_run = device.claim(B, ClaimKind::DirectTaskRun);
    let b_token = b_run
        .granted
        .clone()
        .unwrap_or_else(|| panic!("B's run holds the free device: {b_run:?}"));
    let (_, refused) = device.observe(A);
    assert_refused(&refused, A, "observe_readonly");
    assert!(device.sessions().is_empty(), "nothing opened");
    assert_eq!(device.grants_to(A), 0, "no lease of A");
    device.release(&b_run, &b_token);

    let a_run = device.claim(A, ClaimKind::DirectTaskRun);
    let a_token = a_run
        .granted
        .clone()
        .unwrap_or_else(|| panic!("A's run holds the free device: {a_run:?}"));
    let (_, observed) = device.observe(A);
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true)],
        "A's session stays open while A's run holds the device"
    );
    assert_eq!(device.grants_to(A), 1, "no close lease beside A's run");
    let waiting = device.claim(B, ClaimKind::SelfCheck);
    assert_eq!(
        waiting.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "{waiting:?}"
    );
    device.release(&a_run, &a_token);
    let token = waiting
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted after A's run");
    assert_eq!(
        device.sessions(),
        vec![(A.to_owned(), true), (A.to_owned(), false)]
    );
    let events = all_events(&device.host);
    assert!(first_close(&events, device.ids[A]) < grant_of(&events, &waiting));
    device.release(&waiting, &token);
    device.close();
}

/// The work order's lone instance, compared within one host with and without the slice's branch
/// reachable (`mapp1c_off_for_test`): C, alone on its device beside the group, keeps the session
/// its first observation opened; then, once with the slice on and once with it off, its keyless
/// observation and a lease it takes and releases give the same receipts, the same records (types
/// and severities, in order) and the same device steps (no open, no close), and C's claims are
/// ordered under the same queue-order lock.
#[test]
fn mapp1c_a_lone_instances_observation_and_lease_are_the_same_with_and_without_the_slice_reachable()
{
    let device = Device::start(false);
    let (_, opened) = device.observe(C);
    assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");
    let order = device.host.queue_order_lock_for_test(C).expect("C's lock");
    let c = &device.states[C];
    let mut readings = Vec::new();
    for off in [false, true] {
        device
            .host
            .mapp1c_off_for_test(off)
            .expect("the slice's branch");
        let opens = c.capture_open_count.load(Ordering::Acquire);
        let closes = c.capture_close_count.load(Ordering::Acquire);
        let (observe, observed) = device.observe(C);
        let observation = (
            observed.state(),
            device.shapes(EventQuery {
                correlation_id: Some(observe.correlation_id()),
                ..EventQuery::default()
            }),
        );
        let connection = device.connection();
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
        let Some(RuntimeResult::LeaseGranted { token }) = granted.result() else {
            panic!("C's lease: {granted:?}");
        };
        let release = device.request(RuntimeOperation::ReleaseLease {
            token: token.clone(),
        });
        let released = device
            .host
            .process_request_for_test(&release, connection)
            .expect("C's release");
        let lease = (
            granted.state(),
            device.shapes(EventQuery {
                request_id: Some(acquire.request_id()),
                ..EventQuery::default()
            }),
            released.state(),
            device.shapes(EventQuery {
                request_id: Some(release.request_id()),
                ..EventQuery::default()
            }),
        );
        let steps = (
            c.capture_open_count.load(Ordering::Acquire) - opens,
            c.capture_close_count.load(Ordering::Acquire) - closes,
        );
        let same_lock = Arc::ptr_eq(
            &order,
            &device.host.queue_order_lock_for_test(C).expect("C's lock"),
        );
        readings.push((observation, lease, steps, same_lock));
    }
    device
        .host
        .mapp1c_off_for_test(false)
        .expect("the slice's branch");
    let [on, off] = readings.as_slice() else {
        panic!("two rounds: {readings:#?}");
    };
    assert_eq!(
        on, off,
        "the same with and without the slice's branch reachable"
    );
    assert_eq!(on.0.0, RuntimeReceiptState::Completed);
    assert_eq!(on.2, (0, 0), "C keeps its session: no open, no close");
    assert!(on.3, "C's own queue-order lock");
    assert!(
        device.sessions().is_empty(),
        "the group's device saw nothing"
    );
    device.close();
}

/// The lock walk's pins (register §1.350, "The lock order"): a device's paths take the device's
/// admission guard and then its queue-order lock before the scheduler's lock, and both are the
/// device's own: one for every member of a group of several, as the scheduler's state is one per
/// device, so a member's claim is never granted by another member's pump before its context is
/// registered under that lock; an instance alone on its device has its own of each.
#[test]
fn mapp1c_a_devices_admission_guard_and_queue_order_lock_are_one_per_device() {
    let device = Device::start(false);
    let guard = |alias: &str| {
        device
            .host
            .instance_admission_for_test(alias)
            .expect("the admission guard")
    };
    let order = |alias: &str| {
        device
            .host
            .queue_order_lock_for_test(alias)
            .expect("the queue-order lock")
    };
    assert!(Arc::ptr_eq(&guard(A), &guard(B)), "one admission guard");
    assert!(Arc::ptr_eq(&order(A), &order(B)), "one queue-order lock");
    assert!(!Arc::ptr_eq(&guard(A), &guard(C)));
    assert!(!Arc::ptr_eq(&order(A), &order(C)));
    assert!(!Arc::ptr_eq(&guard(C), &order(C)));
    device.close();
}
