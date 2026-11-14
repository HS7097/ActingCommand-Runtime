// SPDX-License-Identifier: AGPL-3.0-only

//! Workflows #456 and #458 (register §1.363; `contracts/read-session-resource-close.md`;
//! `contracts/device-group.md`, "Release."): a close of one instance's session that is refused
//! before it began isolates that instance, and the Runtime runs on.
//!
//! - #458: a lease that is not a run's in its end phase lapses while its instance's session is
//!   open and must close (two instances on the emulator's native capture, the multi-Nemu gate; a
//!   member of a device group of several). The lease sweep's expiry asks the close's write
//!   authority under the lapsed lease, which `begin_resource_close` refuses (`lease_expired`), and
//!   the sweep used to mark that fatal. Now the instance is fenced by the refusal, the lease ends
//!   handing the device to no one, and its session is closed under the instance's admission guard
//!   right after, under the session close ahead; the claim that waited first, of the same
//!   instance or of another member, is granted only after that close (fix round 1: an instance
//!   alone handed its lapsed lease to its own waiting claim, and that transfer's close met the
//!   lapsed lease again). Fix round 1 also keeps #411 A64's owed close, refused by a claim the
//!   fenced instance's gate holds back, from ending the lease sweep's thread unseen.
//! - #456: a keyless observation's capture fails and its close lease is refused for a reason of
//!   the instance's own key (the takeover cooldown, a queued claim its routine gate holds back,
//!   the lapsed lease the close reused): the observation answers its capture failure, and the
//!   instance alone is fenced.
//!
//! Fixtures: X and Y alone on their devices, both capturing through the Nemu IPC (so each closes
//! its session at every lease end), and LONE alone with no device configuration (it keeps its
//! session); or the members A and B of one device group of several, writing one fake device's
//! session log, whose capture closes must run under a fenced resource-close step. A manual
//! Runtime clock that moves only when a test moves it; stuck recovery off, so a fence waits for a
//! person and no ladder acts. Every expected value is the brief's or the contract's; comparisons
//! across the two hosts of the last test are of normalised payloads, never of identities. Neutral
//! names only.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::gate_411_a64::nemu_capture;
use super::instance_workers::{all_events, phases};
use super::*;
use crate::codes::HostLocation;
use crate::host::{ContainedTaskCheckpointPoint, HostClaimForTest};
use actingcommand_contract::{
    InstanceStuckRecovery, ResourceQuiescence, RuntimeLifecyclePhase, RuntimePayload,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_scheduler::{ClaimKind, QueuedLease};
use serde_json::{Value, json};

/// Alone on its device, on the Nemu IPC.
const X: &str = "close458.instance.x";
/// Alone on its device, on the Nemu IPC: with X, two instances of the multi-Nemu gate.
const Y: &str = "close458.instance.y";
/// Alone on its device with no device configuration: it keeps its session across lease ends.
const LONE: &str = "close458.instance.lone";
/// The members of one device group of several.
const A: &str = "close458.member.a";
const B: &str = "close458.member.b";
const GROUP: &str = "device.one";
/// Bounds a broken run's wait only; nothing sleeps.
const WAIT: Duration = Duration::from_secs(120);
/// How far a test moves the Runtime clock to let a lease lapse: past a client lease's time, a
/// Runtime claim's and a run's end grace.
const LAPSE_MS: u64 = 60 * 60 * 1_000;
/// The lease time `request_host_claim_for_test` gives every claim it asks for.
const CLAIM_TTL_MS: u64 = 60_000;

// ---- The fixture.

struct Fixture {
    /// Shared with a run's checkpoint hook.
    host: Arc<RuntimeHost>,
    clock: Arc<ManualRuntimeClock>,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    log: DeviceSessionLog,
    next_connection: AtomicU64,
    root: TempDir,
}

/// Stuck recovery off for each of `aliases`: a fence waits for a person.
fn stuck_recovery_off(aliases: &[&str]) -> BTreeMap<String, InstanceStuckRecovery> {
    aliases
        .iter()
        .map(|alias| {
            (
                (*alias).to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )
        })
        .collect()
}

/// New ids for X, Y and LONE, X's the smaller of X's and Y's: the lease sweep's scan meets the
/// devices in the order of their keys (an instance alone keys its own id), so X's lapsed lease is
/// always ended before Y's in one tick.
fn alone_ids() -> BTreeMap<&'static str, InstanceId> {
    let (first, second) = (instance_id(), instance_id());
    BTreeMap::from([
        (X, first.min(second)),
        (Y, first.max(second)),
        (LONE, instance_id()),
    ])
}

impl Fixture {
    /// X and Y (the multi-Nemu gate) and LONE, each alone on its device, with the ids `ids`. With
    /// `cooling`, LONE's device starts in the takeover cooldown, as after an unclean end that
    /// listed it; the cooldown lasts until the test moves the clock.
    fn alone(ids: &BTreeMap<&'static str, InstanceId>, cooling: bool) -> Self {
        let root = TempDir::new().expect("tempdir");
        let states = [X, Y, LONE]
            .into_iter()
            .map(|alias| (alias, Arc::new(FakeState::default())))
            .collect::<BTreeMap<_, _>>();
        for alias in [X, Y] {
            states[alias]
                .require_fenced_capture_close
                .store(true, Ordering::Release);
        }
        let provider = FakeProvider::from_entries(
            states
                .iter()
                .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
                .collect::<Vec<_>>(),
        );
        let x = provider
            .resolve(X)
            .expect("X")
            .with_configuration(nemu_capture("TESTSERIAL0001"));
        let y = provider
            .resolve(Y)
            .expect("Y")
            .with_configuration(nemu_capture("TESTSERIAL0002"));
        let provider = provider.with_resolved_for(X, x).with_resolved_for(Y, y);
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let mut config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(stuck_recovery_off(&[X, Y, LONE]));
        if cooling {
            config = config.with_takeover_cooldown_for_test([ids[LONE]]);
        }
        let host = RuntimeHost::start(config, Arc::new(provider)).expect("start the host");
        Self {
            host: Arc::new(host),
            clock,
            ids: ids.clone(),
            states,
            log: DeviceSessionLog::default(),
            next_connection: AtomicU64::new(4_580),
            root,
        }
    }

    /// A and B, one device group of several, writing one session log.
    fn group() -> Self {
        let root = TempDir::new().expect("tempdir");
        let ids = BTreeMap::from([(A, instance_id()), (B, instance_id())]);
        let device = ids[A].min(ids[B]);
        let states = [A, B]
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
            .with_stuck_recovery(stuck_recovery_off(&[A, B]))
            .with_device_keys(BTreeMap::from([(ids[A], device), (ids[B], device)]))
            .with_device_group_names(BTreeMap::from([
                (A.to_owned(), GROUP.to_owned()),
                (B.to_owned(), GROUP.to_owned()),
            ]));
        let provider = FakeProvider::from_entries(
            states
                .iter()
                .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
                .collect::<Vec<_>>(),
        );
        let host = RuntimeHost::start(config, Arc::new(provider)).expect("start the group's host");
        Self {
            host: Arc::new(host),
            clock,
            ids,
            states,
            log,
            next_connection: AtomicU64::new(4_580),
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

    /// `operation` processed on `connection`.
    fn process_on(&self, operation: RuntimeOperation, connection: ConnectionId) -> RuntimeReceipt {
        self.host
            .process_request_for_test(&self.request(operation), connection)
            .expect("the request's receipt")
    }

    /// One keyless read-only observation of `alias`, its request and its receipt.
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

    /// A client's lease of `alias` on `connection`.
    fn acquire_on(&self, alias: &str, connection: ConnectionId) -> LeaseToken {
        let holder = IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder");
        let receipt = self.process_on(RuntimeOperation::acquire_lease(alias, holder), connection);
        let Some(RuntimeResult::LeaseGranted { token }) = receipt.result() else {
            panic!("{alias}'s lease: {receipt:?}");
        };
        token.clone()
    }

    /// A Runtime claim of `kind` on `alias` (60 s of lease time).
    fn claim(&self, alias: &str, kind: ClaimKind) -> HostClaimForTest {
        self.host
            .request_host_claim_for_test(alias, kind)
            .expect("a claim")
    }

    /// A scheduling pause of `alias`, as a client asks for it.
    fn pause(&self, alias: &str) {
        let mut client = TestClient::connect(&self.host);
        let pause = client.request(RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: alias.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("close456_instance_pause")
                .expect("pause reason"),
            drain_timeout_ms: 1_000,
        });
        let receipt = client.send(&pause);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// The business capacity sampled now, as the capacity monitor samples it while time passes.
    fn sample_capacity(&self) {
        let sample = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample().expect("capacity sample");
    }

    /// The Runtime clock moved past every lease the test holds.
    fn lapse(&self) {
        self.clock.advance(LAPSE_MS);
        self.sample_capacity();
    }

    /// One tick of the lease sweep (its expiry scan, the queues' pump).
    fn tick(&self) {
        self.host
            .expire_due_leases_for_test()
            .expect("one sweep tick");
    }

    fn head(&self) -> u64 {
        all_events(&self.host)
            .last()
            .map_or(0, PersistedEvent::sequence)
    }

    fn events_after(&self, after: u64) -> Vec<PersistedEvent> {
        all_events(&self.host)
            .into_iter()
            .filter(|event| event.sequence() > after)
            .collect()
    }

    /// Nothing latched and no stop was requested.
    fn assert_runtime_runs(&self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime latched"
        );
        assert!(
            !self
                .host
                .is_shutdown_requested()
                .expect("the shutdown state"),
            "a stop was requested"
        );
    }

    /// Only `fenced` is fenced, with `cause`, in the status and in one fence record of those
    /// after `after`, which names it.
    fn assert_only_fenced(&self, fenced: &str, cause: &str, after: u64) {
        for alias in self.ids.keys() {
            let fence = status_fence(&self.host, alias);
            if *alias == fenced {
                let fence = fence.unwrap_or_else(|| panic!("{alias}'s fence"));
                assert_eq!(fence.cause.as_str(), cause, "{fence:?}");
            } else {
                assert_eq!(fence, None, "{alias} is not fenced");
            }
        }
        let events = self.events_after(after);
        let fences = fence_records(&events);
        let [(record, outcome)] = fences.as_slice() else {
            panic!("one fence record: {fences:#?}");
        };
        assert_eq!(record.links().instance_id(), Some(&self.ids[fenced]));
        assert_fence(outcome, self.ids[fenced], cause);
    }

    /// Whether a lease of `alias` was handed on by a transfer after `after`.
    fn transferred_since(&self, after: u64, alias: &str) -> bool {
        self.events_after(after).iter().any(|event| {
            event.event_type() == EventType::LeaseTransferred
                && event.links().instance_id() == Some(&self.ids[alias])
        })
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
            assert!(
                open.values().filter(|count| **count > 0).count() <= 1,
                "step {step}: two members' sessions open on the device at once: {entries:?}"
            );
        }
        entries
    }

    /// The host, once every hook that shared it has run.
    fn into_host(self) -> RuntimeHost {
        let Ok(host) = Arc::try_unwrap(self.host) else {
            panic!("a hook still holds the host");
        };
        host
    }

    fn close(self) {
        self.assert_runtime_runs();
        assert!(
            self.host.sweep_thread_runs_for_test(),
            "the lease sweep's thread still runs"
        );
        self.into_host().close().expect("close the host");
    }

    /// The host closed without asserting how: a session a fence keeps open, or the Runtime's
    /// own failure, is not the subject of the test that ends so.
    fn close_unchecked(self) {
        let _closed = self.into_host().close();
    }

    /// The neutral package of a direct run written beside the host, the fake device of `alias`
    /// set to finish it on its target page (`gate_mapp1d.rs`'s).
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
}

/// The `runtime.failed` records in `events` whose outcome's code is `wanted`.
fn failures<'a>(events: &'a [PersistedEvent], wanted: &str) -> Vec<&'a PersistedEvent> {
    events
        .iter()
        .filter(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record)) => {
                record.outcome().code().as_str() == wanted
            }
            _ => false,
        })
        .collect()
}

/// The outcome of the `runtime.failed` record `event`.
fn failure_outcome(event: &PersistedEvent) -> &actingcommand_contract::outcome::Outcome {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record)) => record.outcome(),
        other => panic!("a runtime.failed record: {other:?}"),
    }
}

/// The records of `event_type` in `events` linked to `lease_id`.
fn lease_records(
    events: &[PersistedEvent],
    event_type: EventType,
    lease_id: LeaseId,
) -> Vec<&PersistedEvent> {
    events
        .iter()
        .filter(|event| {
            event.event_type() == event_type && event.links().lease_id() == Some(&lease_id)
        })
        .collect()
}

/// The sequences of the confirmed resource closes of `instance` in `events`.
fn confirmed_closes(events: &[PersistedEvent], instance: InstanceId) -> Vec<u64> {
    phases(events)
        .into_iter()
        .filter_map(|(event, phase)| match phase {
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id,
                quiescence: ResourceQuiescence::Confirmed,
                ..
            } if instance_id == instance => Some(event.sequence()),
            _ => None,
        })
        .collect()
}

/// The sequence of the `lease.granted` of `claim` in `events`.
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

// ---- The comparison of the last test: two hosts, the slices on and off.

/// Whether a member is named `key`, whose value is a time or a place (an instant on either clock,
/// a measurement, a ledger position, an evaluation's `seed`), which two hosts hold apart however
/// alike they are.
fn time_or_place(key: &str) -> bool {
    key.ends_with("_unix_ms")
        || key.ends_with("_monotonic_ms")
        || key.ends_with("_us")
        || key.ends_with("_at")
        || matches!(
            key,
            "duration_ms" | "local_day" | "sequence" | "record_index" | "input_ledger_position"
        )
        || key == "seed"
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

/// `text` with the temporary root's base name `root` replaced, and each run of 32 or more
/// lowercase hexadecimal digits (a minted id, a digest).
fn without_identities(text: &str, root: &str) -> String {
    let text = text.replace(root, "<root>");
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

/// `value` with its ids, times, places and the root's base name normalised.
fn normalised(value: &mut Value, root: &str) {
    match value {
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                if time_or_place(key) {
                    *member = json!("<normalised>");
                } else {
                    normalised(member, root);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| normalised(item, root)),
        Value::String(text) => *text = without_identities(text, root),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// One record as the comparison reads it.
#[derive(Debug, PartialEq)]
struct Compared {
    event_type: EventType,
    severity: EventSeverity,
    instance: Option<InstanceId>,
    payload: Value,
}

/// Every difference between the two hosts' records of one part, named in one list.
fn differences(part: &str, on: &[Compared], off: &[Compared]) -> Vec<String> {
    let mut found = Vec::new();
    if on.len() != off.len() {
        found.push(format!(
            "{part}: {} records on, {} off",
            on.len(),
            off.len()
        ));
    }
    for (index, (with, without)) in on.iter().zip(off).enumerate() {
        if with != without {
            found.push(format!(
                "{part}, record {index}: on {with:?}, off {without:?}"
            ));
        }
    }
    found
}

impl Fixture {
    /// The records after `after` that name `alias`, but a frame's retention records, which the
    /// frame cleaner writes on its own time, as the comparison reads them.
    fn compared_since(&self, after: u64, alias: &str) -> Vec<Compared> {
        let root = self
            .root
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("the temporary root's base name")
            .to_owned();
        self.events_after(after)
            .into_iter()
            .filter(|event| {
                event.links().instance_id() == Some(&self.ids[alias])
                    && event.payload().artifact_retention().is_none()
            })
            .map(|event| {
                let mut payload =
                    serde_json::to_value(event.payload()).expect("a record's payload encodes");
                normalised(&mut payload, &root);
                Compared {
                    event_type: event.event_type(),
                    severity: event.severity(),
                    instance: event.links().instance_id().copied(),
                    payload,
                }
            })
            .collect()
    }

    /// The three lease ends of the comparison, none of whose closes is refused, each one's
    /// records: LONE's lease lapses with its session open and the expiry keeps it; X's lease,
    /// with X's session open, is released and the release closes it; Y's lease lapses with no
    /// session open and the expiry has nothing to close.
    fn unrefused_lease_ends(&self) -> [Vec<Compared>; 3] {
        let connection = self.connection();
        // LONE: the expiry keeps the session.
        let opened = self.observe(LONE).1;
        assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");
        let lone_closes = self.states[LONE]
            .capture_close_count
            .load(Ordering::Acquire);
        let lone = self.acquire_on(LONE, connection);
        let mark = self.head();
        self.lapse();
        self.tick();
        assert_eq!(
            lease_records(
                &all_events(&self.host),
                EventType::LeaseExpired,
                lone.lease_id()
            )
            .len(),
            1,
            "LONE's lease expired"
        );
        assert_eq!(
            self.states[LONE]
                .capture_close_count
                .load(Ordering::Acquire),
            lone_closes,
            "LONE keeps its session"
        );
        let kept = self.compared_since(mark, LONE);

        // X: the release closes the session.
        self.sample_capacity();
        let opened = self.observe(X).1;
        assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");
        let x_closes = self.states[X].capture_close_count.load(Ordering::Acquire);
        let x = self.acquire_on(X, connection);
        let mark = self.head();
        let released = self.process_on(RuntimeOperation::ReleaseLease { token: x }, connection);
        assert_eq!(
            released.state(),
            RuntimeReceiptState::Completed,
            "{released:?}"
        );
        assert_eq!(
            self.states[X].capture_close_count.load(Ordering::Acquire),
            x_closes + 1,
            "the release closed X's session"
        );
        let closed = self.compared_since(mark, X);

        // Y: the expiry must close and finds nothing open.
        let y = self.acquire_on(Y, connection);
        let mark = self.head();
        self.lapse();
        self.tick();
        assert_eq!(
            lease_records(
                &all_events(&self.host),
                EventType::LeaseExpired,
                y.lease_id()
            )
            .len(),
            1,
            "Y's lease expired"
        );
        let nothing_open = self.compared_since(mark, Y);

        assert!(fence_records(&all_events(&self.host)).is_empty());
        [kept, closed, nothing_open]
    }
}

// ---- The tests.

/// #458 for an instance alone on its device, the sweep's path (every kind of lease that is not a
/// run's in its end phase ends there: a client's lease, a Lab session's, an operator's or the
/// Runtime's own claim; this test drives a client's lease of X and a Runtime claim of Y). X's
/// observation opens X's session and a client's lease of X lapses with it open; Y's Runtime claim
/// lapses while another claim of Y waits; X's id is the smaller, so the scan ends X's lease first.
/// One sweep tick: the Runtime's health stays clean; X alone is fenced, with the refusal
/// `lease_expired` as its cause; the refusal is recorded once, naming X's lease and the stage
/// `runtime_lifecycle_session_close`; X's lease is expired once and X's session is closed after
/// it, under a fenced close; after X's, in the same tick, Y's lapsed lease is handed to Y's
/// waiting claim. In the next tick (the clock moved past Y's handed lease and short of X's
/// fence's recheck) Y's lease lapses again and is handed to Y's next claim, nothing of X runs, and
/// the Runtime and its lease sweep's thread still run.
#[test]
fn close458_a_lease_lapsed_with_its_session_open_fences_only_its_instance_and_the_sweep_goes_on() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let x_opens = fixture.states[X].capture_open_count.load(Ordering::Acquire);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    assert_eq!(
        fixture.states[X].capture_open_count.load(Ordering::Acquire),
        x_opens + 1,
        "X's observation opened X's session"
    );
    let x_lease = fixture.acquire_on(X, fixture.connection());
    let y_claim = fixture.claim(Y, ClaimKind::SelfCheck);
    assert!(y_claim.granted.is_some(), "Y's claim holds Y: {y_claim:?}");
    let y_waiting = fixture.claim(Y, ClaimKind::SelfCheck);
    assert!(
        y_waiting.queued.is_some() && y_waiting.granted.is_none(),
        "Y's second claim waits: {y_waiting:?}"
    );
    let x_closes = fixture.states[X]
        .capture_close_count
        .load(Ordering::Acquire);
    let mark = fixture.head();

    fixture.lapse();
    fixture.tick();

    fixture.assert_runtime_runs();
    let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
    fixture.assert_only_fenced(X, lease_expired, mark);
    let events = fixture.events_after(mark);
    let refusals = failures(&events, lease_expired);
    let [refusal] = refusals.as_slice() else {
        panic!("one record of the refusal: {refusals:#?}");
    };
    let refusal = failure_outcome(refusal);
    assert_eq!(
        refusal.value("lease_id"),
        Some(&serde_json::to_value(x_lease.lease_id()).expect("lease id")),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.value("stage").and_then(Value::as_str),
        Some(HostLocation::RuntimeLifecycleSessionClose.as_str()),
        "{refusal:?}"
    );
    let expiries = lease_records(&events, EventType::LeaseExpired, x_lease.lease_id());
    let [expiry] = expiries.as_slice() else {
        panic!("X's lease expired once: {expiries:#?}");
    };
    let closes = confirmed_closes(&events, fixture.ids[X]);
    assert!(
        closes.len() == 1 && closes[0] > expiry.sequence(),
        "X's session is closed once, after its lease expired: {closes:?}"
    );
    assert_eq!(
        fixture.states[X]
            .capture_close_count
            .load(Ordering::Acquire),
        x_closes + 1,
        "X's session is closed"
    );
    assert_eq!(
        fixture.states[X]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0,
        "under a fenced resource-close step"
    );
    assert!(
        fixture.transferred_since(mark, Y),
        "in the same tick Y's lapsed lease is handed to Y's waiting claim"
    );
    let handed = y_waiting
        .wait_for_grant(WAIT)
        .expect("the grant slot")
        .expect("Y's waiting claim holds Y");
    assert_eq!(handed.instance_id(), fixture.ids[Y]);

    // The next tick: the clock moves just past the lease Y's waiting claim was handed (a claim's
    // time, from the first tick), and stays inside the first wait of X's fence for its recheck
    // (`recovery_ladder.fence_recheck.interval_ms`, 300 000 ms), so no recheck of X runs meanwhile.
    let y_next = fixture.claim(Y, ClaimKind::SelfCheck);
    assert!(y_next.queued.is_some(), "Y's next claim waits: {y_next:?}");
    let mark = fixture.head();
    fixture.clock.advance(CLAIM_TTL_MS + 1);
    fixture.sample_capacity();
    fixture.tick();
    fixture.assert_runtime_runs();
    assert!(
        fixture.events_after(mark).iter().all(|event| {
            event.links().instance_id() != Some(&fixture.ids[X])
                || event.payload().artifact_retention().is_some()
        }),
        "nothing of X ran in the next tick"
    );
    assert!(
        fixture.transferred_since(mark, Y),
        "in the next tick Y's lapsed lease is handed to Y's next claim"
    );
    let last = y_next
        .wait_for_grant(WAIT)
        .expect("the grant slot")
        .expect("Y's next claim holds Y");
    fixture
        .host
        .release_host_claim_for_test(&y_next, &last)
        .expect("Y's last lease end");
    fixture.close();
}

/// Fix round 1: #458 for an instance alone with a claim of its own waiting behind its lapsed
/// lease, one the routine hold does not stop: a person's self-check, then (second round) a direct
/// run's claim. X's observation opens X's session and a client's lease of X holds X with that
/// claim queued behind it; Y's Runtime claim holds Y with another claim of Y queued. One sweep
/// tick: the Runtime's health stays clean; X alone is fenced (`lease_expired`); X's lease expires
/// once and hands the device to no one; X's session is closed once after that, under a fenced
/// close, before X's waiting claim is granted, which the queue's own grant grants (requested,
/// queued, admitted, granted; no transfer); in the same tick Y's lapsed lease is handed to Y's
/// waiting claim. Before the fix the expiry handed X's lease to X's waiting claim, the transfer
/// closed again under the lapsed lease and the Runtime stopped.
#[test]
fn close458_an_instance_alones_lapsed_lease_closes_its_session_before_its_own_waiting_claim() {
    for kind in [ClaimKind::SelfCheck, ClaimKind::DirectTaskRun] {
        let fixture = Fixture::alone(&alone_ids(), false);
        let observed = fixture.observe(X).1;
        assert_eq!(
            observed.state(),
            RuntimeReceiptState::Completed,
            "{kind:?}: {observed:?}"
        );
        let x_lease = fixture.acquire_on(X, fixture.connection());
        let x_waiting = fixture.claim(X, kind);
        assert!(
            x_waiting.queued.is_some() && x_waiting.granted.is_none(),
            "{kind:?}: X's claim waits behind X's lease: {x_waiting:?}"
        );
        let y_claim = fixture.claim(Y, ClaimKind::SelfCheck);
        assert!(y_claim.granted.is_some(), "{kind:?}: {y_claim:?}");
        let y_waiting = fixture.claim(Y, ClaimKind::SelfCheck);
        assert!(y_waiting.queued.is_some(), "{kind:?}: {y_waiting:?}");
        let x_closes = fixture.states[X]
            .capture_close_count
            .load(Ordering::Acquire);
        let mark = fixture.head();

        fixture.lapse();
        fixture.tick();

        fixture.assert_runtime_runs();
        fixture.assert_only_fenced(X, RuntimeErrorCode::LeaseExpired.as_str(), mark);
        let x_token = x_waiting
            .wait_for_grant(WAIT)
            .expect("the grant slot")
            .unwrap_or_else(|| panic!("{kind:?}: X's waiting claim is granted after X's close"));
        assert_eq!(x_token.instance_id(), fixture.ids[X], "{kind:?}");
        let events = fixture.events_after(mark);
        let expiries = lease_records(&events, EventType::LeaseExpired, x_lease.lease_id());
        let [expiry] = expiries.as_slice() else {
            panic!("{kind:?}: X's lease expired once: {expiries:#?}");
        };
        let closes = confirmed_closes(&events, fixture.ids[X]);
        let [close] = closes.as_slice() else {
            panic!("{kind:?}: X's session is closed once: {closes:?}");
        };
        assert!(
            *close > expiry.sequence() && *close < grant_of(&events, &x_waiting),
            "{kind:?}: X's session is closed after X's lease expired and before X's claim's grant"
        );
        assert_eq!(
            fixture.states[X]
                .capture_close_count
                .load(Ordering::Acquire),
            x_closes + 1,
            "{kind:?}"
        );
        assert_eq!(
            fixture.states[X]
                .unfenced_capture_close_count
                .load(Ordering::Acquire),
            0,
            "{kind:?}: under a fenced resource-close step"
        );
        let types = events
            .iter()
            .filter(|event| event.links().request_id() == Some(&x_waiting.request.request_id()))
            .map(PersistedEvent::event_type)
            .collect::<Vec<_>>();
        assert_events_in_order(
            &types,
            &[EventType::SchedulerAdmitted, EventType::LeaseGranted],
        );
        assert!(
            !types.contains(&EventType::LeaseTransferred),
            "{kind:?}: no transfer handed X's claim the device: {types:?}"
        );
        assert!(
            fixture.transferred_since(mark, Y),
            "{kind:?}: in the same tick Y's lapsed lease is handed to Y's waiting claim"
        );
        let y_token = y_waiting
            .wait_for_grant(WAIT)
            .expect("the grant slot")
            .unwrap_or_else(|| panic!("{kind:?}: Y's waiting claim holds Y"));
        fixture
            .host
            .release_host_claim_for_test(&x_waiting, &x_token)
            .expect("X's claim's lease end");
        fixture
            .host
            .release_host_claim_for_test(&y_waiting, &y_token)
            .expect("Y's claim's lease end");
        fixture.close();
    }
}

/// Fix round 1: the lease sweep's thread does not end unseen when #411 A64's owed close is refused
/// `lease_busy`. X's direct run reaches its end work (its end phase entered, its lease renewed by
/// the end grace); there a startup claim of X queues behind the run, the clock moves past the end
/// grace and one sweep tick expires the run's lease: its close meets the lapsed lease and fences X
/// (#411 A64, unchanged), and the close A64 then owes is refused before it begins, `lease_busy`
/// (X's startup claim waits in X's queue, held back by X's fence). That refusal is X's: the tick
/// returns no error, the refusal is recorded once naming X with the stage
/// `runtime_lifecycle_session_close`, the Runtime's health is clean and the lease sweep's thread
/// still runs; a later tick (the clock moved past a claim of Y and short of X's fence's recheck)
/// still ends Y's lapsed lease. The session the refused close left open
/// is closed by the fence's way out: a person's self-check of X closes it first and, passing,
/// lifts the fence, and X's startup claim is then granted. Before the fix the refusal reached the
/// sweep, whose fatal mark refuses a failure that is not fatal, and the sweep's thread ended with
/// nothing latched.
#[test]
fn close458_a64s_owed_close_refused_by_a_held_claim_leaves_the_sweep_running() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let package = fixture.run_package(X);
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &issuer,
        RuntimeOperation::run_contained_task(
            X,
            issuer.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    );
    let (sent, lapsed) = mpsc::channel();
    let host = Arc::clone(&fixture.host);
    let clock = Arc::clone(&fixture.clock);
    let checkpoint = fixture
        .host
        .run_at_request_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            run.request_id(),
            fixture.ids[X],
            move |_| {
                let held = host
                    .request_host_claim_for_test(X, ClaimKind::StartupPackage)
                    .expect("X's startup claim");
                clock.advance(LAPSE_MS);
                let swept = host.expire_due_leases_for_test();
                sent.send((held, swept)).expect("the lapse");
            },
        )
        .expect("arm X's end work");

    let receipt = fixture
        .host
        .process_request_for_test(&run, fixture.connection())
        .expect("X's run's receipt");

    assert_eq!(checkpoint.consumed(), 1, "X's run reached its end work");
    let (held, swept) = lapsed.recv_timeout(WAIT).expect("the lapse");
    assert!(swept.is_ok(), "the sweep's tick: {swept:?}");
    assert!(
        receipt
            .error_projection()
            .is_none_or(|projection| !projection.fatal),
        "{receipt:?}"
    );
    assert!(
        held.queued.is_some() && held.granted.is_none(),
        "X's startup claim waited behind the run: {held:?}"
    );
    fixture.assert_runtime_runs();
    assert!(
        fixture.host.sweep_thread_runs_for_test(),
        "the lease sweep's thread still runs"
    );
    let fence = status_fence(&fixture.host, X).expect("X's fence");
    assert_eq!(
        fence.cause.as_str(),
        RuntimeErrorCode::LeaseExpired.as_str(),
        "{fence:?}"
    );
    let events = all_events(&fixture.host);
    let refusals = failures(&events, RuntimeErrorCode::LeaseBusy.as_str());
    let [refused] = refusals.as_slice() else {
        panic!("one record of the refused close lease: {refusals:#?}");
    };
    assert_eq!(refused.links().instance_id(), Some(&fixture.ids[X]));
    assert_eq!(
        failure_outcome(refused)
            .value("stage")
            .and_then(Value::as_str),
        Some(HostLocation::RuntimeLifecycleSessionClose.as_str())
    );
    assert_eq!(
        fixture
            .host
            .instance_claims_for_test(X)
            .expect("X's claims"),
        (None, vec![ClaimKind::StartupPackage]),
        "X is free and its startup claim waits, held"
    );

    // A later tick still ends another instance's lapsed lease. The clock moves only past that
    // claim's time, inside the first wait of X's fence for its recheck (300 000 ms), so no recheck
    // of X runs meanwhile.
    fixture.sample_capacity();
    let y_claim = fixture.claim(Y, ClaimKind::SelfCheck);
    let y_lease = y_claim
        .granted
        .clone()
        .unwrap_or_else(|| panic!("Y's claim holds Y: {y_claim:?}"));
    fixture.clock.advance(CLAIM_TTL_MS + 1);
    fixture.sample_capacity();
    fixture.tick();
    assert_eq!(
        lease_records(
            &all_events(&fixture.host),
            EventType::LeaseExpired,
            y_lease.lease_id()
        )
        .len(),
        1,
        "Y's lapsed lease is ended"
    );
    fixture.assert_runtime_runs();
    assert!(fixture.host.sweep_thread_runs_for_test());

    // The fence's way out: a person's self-check closes the session and lifts the fence.
    let x_closes = fixture.states[X]
        .capture_close_count
        .load(Ordering::Acquire);
    let checked = fixture.process_on(
        RuntimeOperation::SelfCheckInstance {
            instance_alias: X.to_owned(),
        },
        fixture.connection(),
    );
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    assert!(
        fixture.states[X]
            .capture_close_count
            .load(Ordering::Acquire)
            > x_closes,
        "the self-check closed the session the refused close left open"
    );
    assert_eq!(
        status_fence(&fixture.host, X),
        None,
        "the passing self-check lifted X's fence"
    );
    let token = held
        .wait_for_grant(WAIT)
        .expect("the grant slot")
        .expect("X's startup claim is granted once X's fence is lifted");
    fixture
        .host
        .release_host_claim_for_test(&held, &token)
        .expect("the startup claim's lease end");
    fixture.close();
}

/// #458 on a device of several: A's Runtime claim holds the device, A's observation under it opens
/// A's session, and B's claim waits for the device; A's lease lapses with A's session open. One
/// sweep tick: the Runtime's health stays clean; A alone is fenced (`lease_expired`); the expiry
/// hands the device to no one, A's session is closed under the session close ahead, fenced and
/// confirmed, and only then is B's claim granted, by the queue's own grant (requested, queued,
/// admitted, granted; no transfer) with B's member; at no step of the device's log are two
/// members' sessions open.
#[test]
fn close458_a_members_lapsed_lease_hands_the_device_on_only_after_its_session_is_closed() {
    let fixture = Fixture::group();
    let a_claim = fixture.claim(A, ClaimKind::SelfCheck);
    let a_lease = a_claim
        .granted
        .clone()
        .unwrap_or_else(|| panic!("A's claim holds the free device: {a_claim:?}"));
    let observed = fixture.observe(A).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    assert!(
        open_backends(&fixture.sessions(), A) > 0,
        "A's observation under A's own lease keeps A's session open"
    );
    let b_claim = fixture.claim(B, ClaimKind::SelfCheck);
    assert_eq!(
        b_claim.queued.as_ref().map(QueuedLease::position),
        Some(1),
        "B's claim waits for the device: {b_claim:?}"
    );
    let mark = fixture.head();

    fixture.lapse();
    fixture.tick();

    fixture.assert_runtime_runs();
    fixture.assert_only_fenced(A, RuntimeErrorCode::LeaseExpired.as_str(), mark);
    let b_lease = b_claim
        .wait_for_grant(WAIT)
        .expect("the grant slot")
        .expect("B's claim is granted once A's session is closed");
    assert_eq!(b_lease.instance_id(), fixture.ids[B]);
    let events = all_events(&fixture.host);
    let expiries = lease_records(&events, EventType::LeaseExpired, a_lease.lease_id());
    let [expiry] = expiries.as_slice() else {
        panic!("A's lease expired once: {expiries:#?}");
    };
    let closes = confirmed_closes(&events, fixture.ids[A])
        .into_iter()
        .filter(|sequence| *sequence > expiry.sequence())
        .collect::<Vec<_>>();
    let [close] = closes.as_slice() else {
        panic!("A's session is closed once after its lease expired: {closes:?}");
    };
    assert!(
        *close < grant_of(&events, &b_claim),
        "A's close is recorded before B's lease.granted"
    );
    let types = all_events(&fixture.host)
        .iter()
        .filter(|event| event.links().request_id() == Some(&b_claim.request.request_id()))
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
        "no transfer handed B the device: {types:?}"
    );
    let log = fixture.sessions();
    assert_eq!(open_backends(&log, A), 0, "A's session is closed: {log:?}");
    assert_eq!(
        fixture.states[A]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0,
        "A's capture closed under a fenced resource-close step"
    );
    fixture
        .host
        .release_host_claim_for_test(&b_claim, &b_lease)
        .expect("B's lease end");
    fixture.close();
}

/// Why a keyless observation's close lease is refused in the #456 test.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The device's takeover cooldown (`lease_cooldown`).
    Cooldown,
    /// A queued claim of the instance that its own routine gate holds back (`lease_busy`).
    HeldClaim,
}

/// #456: LONE's first observation opens its session; then its capture fails, and the capture
/// failure's dedicated close lease is refused, in the device's takeover cooldown (`lease_cooldown`)
/// and, out of it, by LONE's own startup claim that LONE's scheduling pause holds back in the
/// queue (a queue ahead, `lease_busy`); the third reason, the lapsed lease the close reuses, is
/// `gate_411_a52`'s T2. Each time: the observation answers its own capture failure as a capture
/// failure whose close a waiting entry refused does (failed, `capture_failed`, not fatal); the
/// Runtime's health stays clean; LONE alone is fenced, with the refusal as its cause; the refusal
/// is recorded once under the observation's request, naming LONE.
#[test]
fn close456_a_keyless_observations_close_lease_refused_for_its_own_key_fences_only_its_instance() {
    for refusal in [Refusal::Cooldown, Refusal::HeldClaim] {
        let fixture = Fixture::alone(&alone_ids(), matches!(refusal, Refusal::Cooldown));
        let opened = fixture.observe(LONE).1;
        assert_eq!(
            opened.state(),
            RuntimeReceiptState::Completed,
            "{refusal:?}: {opened:?}"
        );
        let code = match refusal {
            Refusal::Cooldown => RuntimeErrorCode::LeaseCooldown.as_str(),
            Refusal::HeldClaim => {
                fixture.pause(LONE);
                let held = fixture.claim(LONE, ClaimKind::StartupPackage);
                assert!(
                    held.queued.is_some() && held.granted.is_none(),
                    "LONE's startup claim waits, held back by its pause: {held:?}"
                );
                RuntimeErrorCode::LeaseBusy.as_str()
            }
        };
        let state = &fixture.states[LONE];
        state.fail_capture.store(true, Ordering::Release);
        state
            .transient_capture_failure
            .store(true, Ordering::Release);
        let mark = fixture.head();

        let (request, failed) = fixture.observe(LONE);

        assert_eq!(
            failed.state(),
            RuntimeReceiptState::Failed,
            "{refusal:?}: {failed:?}"
        );
        let projection = failed
            .error_projection()
            .expect("the observation's failure");
        assert!(!projection.fatal, "{refusal:?}: {failed:?}");
        assert_eq!(
            projection.code,
            RuntimeErrorCode::CaptureFailed,
            "{refusal:?}: {failed:?}"
        );
        fixture.assert_runtime_runs();
        fixture.assert_only_fenced(LONE, code, mark);
        let events = fixture.events_after(mark);
        let refusals = failures(&events, code);
        let [refused] = refusals.as_slice() else {
            panic!("{refusal:?}: one record of the refusal: {refusals:#?}");
        };
        assert_eq!(
            refused.links().request_id(),
            Some(&request.request_id()),
            "{refusal:?}"
        );
        assert_eq!(
            refused.links().instance_id(),
            Some(&fixture.ids[LONE]),
            "{refusal:?}"
        );
        // The session the fence keeps open is closed by the fence's own paths (its recheck, a
        // person's self-check), which this test does not drive; the Host close's own close of it
        // is not its subject.
        fixture.close_unchecked();
    }
}

/// A failure of the Runtime itself in these paths stays fatal: X's client lease lapses with X's
/// session open, and the record of the refused close that would fence X cannot be appended (the
/// ledger's failure, injected by a test-only switch that holds until the host ends). The sweep's
/// tick returns that failure, fatal and Global, which the resident lease sweep latches
/// (`lease_sweep_loop` marks what its tick returns): `ledger_failure` at the fence. The resident
/// sweep serializes its ticks with this test's, so either this tick returns it or the resident
/// one met the lapsed lease first and the Runtime holds it latched; nothing is waited for. No
/// instance is fenced.
#[test]
fn close458_a_record_that_cannot_be_appended_at_the_expirys_close_still_stops_the_runtime() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let _lapsing = fixture.acquire_on(X, fixture.connection());
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");

    fixture.lapse();
    let tick = fixture.host.expire_due_leases_for_test();

    let failure = match fixture.host.fatal_error().expect("runtime health") {
        Some(latched) => latched,
        None => tick.expect_err("the expiry's tick returns the record's failure"),
    };
    assert_eq!(
        failure.code(),
        RuntimeErrorCode::LedgerFailure.as_str(),
        "{failure:?}"
    );
    assert_eq!(
        failure.operation(),
        HostLocation::FenceInstanceResources.as_str(),
        "{failure:?}"
    );
    assert!(failure.is_fatal() && failure.halts_runtime(), "{failure:?}");
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    fixture.close_unchecked();
}

/// No refusal, nothing changes. Two hosts of this build started alike with the same instance
/// ids, one with the two slices turned off by the test-only switch (`close_isolates_off_for_test`,
/// compiled under `cfg(test)` only: it turns off the entries the slices add, the fence of a
/// refused close at a lease's expiry and after a capture failure, and with them the owed close and
/// the hand-over it holds back). On each: LONE's lease lapses with LONE's session open and the
/// expiry keeps the session; X's lease, with X's session open, is released and the release closes
/// the session; Y's lease lapses with no session open and the expiry closes nothing. The records
/// of each lease end that name its instance are equal on the two hosts record by record, by type,
/// severity, instance and whole payload, with ids, times, places, an evaluation's `seed` and the
/// temporary root's base name normalised; every difference is named in one failure. No instance
/// is fenced on either host.
///
/// What this proves is narrow (fix round 1, the review): none of the three lease ends meets a
/// refused close, so neither entry the switch guards is reached on either host, and the additions
/// that run on every lease end (the owed set's two reads, the scan's skip of a lease a holder
/// ended meanwhile) run on both hosts alike. It proves that these three lease ends write the same
/// records on this build as on itself with the entries off, and that the additions write no record
/// there; that nothing changes against main when no close is refused rests on reading the diff
/// (register §1.363).
#[test]
fn close458_without_a_refusal_every_lease_end_writes_the_records_it_wrote_before() {
    let ids = alone_ids();
    let rounds = [false, true].map(|off| {
        let fixture = Fixture::alone(&ids, false);
        fixture
            .host
            .close_isolates_off_for_test(off)
            .expect("the slices' switch");
        let records = fixture.unrefused_lease_ends();
        fixture.close();
        records
    });
    let [on, off] = rounds;
    let mut found = Vec::new();
    for (part, (with, without)) in [
        "kept at its expiry",
        "closed at its release",
        "nothing open",
    ]
    .iter()
    .zip(on.iter().zip(off.iter()))
    {
        found.extend(differences(part, with, without));
    }
    assert!(
        found.is_empty(),
        "the slices changed records of lease ends whose close no one refused: {found:#?}"
    );
}
