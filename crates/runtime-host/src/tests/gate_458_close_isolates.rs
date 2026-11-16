// SPDX-License-Identifier: AGPL-3.0-only

//! Workflows #456 and #458 (register §1.364; `contracts/read-session-resource-close.md`;
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
//! - #461 (register §1.431): two paths meet the lapsed lease before the sweep does. A client's
//!   connection closes after its lease lapsed, and the connection's clean-up closes the session
//!   under it; an input is under way when its lease lapses, the input fails, and its clean-up
//!   closes the session under it. Each refusal used to stop the Runtime (the second also kept the
//!   owner as unconfirmed); now each fences the instance as #458's expiry does, and the close is
//!   made once the lease has ended.
//! - #475 (register §1.432): five more closes under a key whose lease lapsed before the sweep met
//!   it (a failed run's hand-over to its ladder, a pre-emption's hand-over, an application
//!   command's preparation, a backend switch, a member switch) used to stop the Runtime; now each
//!   fences the instance whose session it is, the key goes to no one, and the key's end makes the
//!   close. A test hook lets the lease lapse right before such a close asks its write authority,
//!   as a device call that outlasts the lease lets it lapse there.
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
use super::instance_workers::{all_events, ladder_setup, phases};
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::host::{ContainedTaskCheckpointPoint, HostClaimForTest};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, InstanceStuckRecovery,
    LeasePayload, OwnerResourceDisposition, ResourceQuiescence, RuntimeConfigManifest,
    RuntimeLifecyclePhase, RuntimePayload, SchedulerPayload, SchedulingPauseReason,
    SchedulingPauseScope,
};
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use actingcommand_scheduler::{ClaimKind, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS, QueuedLease};
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
const GATE_WAIT: Duration = Duration::from_secs(120);
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

    /// Closes the host, once every hook that shared it has run, and only then drops its state
    /// root, so the host never closes over a root already gone.
    fn close_host(self) -> RuntimeHostResult<()> {
        let Self { host, root, .. } = self;
        let Ok(host) = Arc::try_unwrap(host) else {
            panic!("a hook still holds the host");
        };
        let closed = host.close();
        drop(root);
        closed
    }

    fn close(self) {
        self.assert_runtime_runs();
        assert!(
            self.host.sweep_thread_runs_for_test(),
            "the lease sweep's thread still runs"
        );
        self.close_host().expect("close the host");
    }

    /// The host closed without asserting how: a session a fence keeps open, or the Runtime's
    /// own failure, is not the subject of the test that ends so.
    fn close_unchecked(self) {
        let _closed = self.close_host();
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
        .wait_for_grant(GATE_WAIT)
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
        .wait_for_grant(GATE_WAIT)
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
            .wait_for_grant(GATE_WAIT)
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
            .wait_for_grant(GATE_WAIT)
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
/// still ends Y's lapsed lease. The session the refused close left open is closed by the fence's
/// way out: a person's self-check of X closes it in its preparation (whether that self-check then
/// passes, and lifts the fence, is the fixture's: on these Nemu-configured fakes it does not, CI
/// run 43558842328, so the test asserts the close and not the lift). Before the fix the refusal
/// reached the sweep, whose fatal mark refuses a failure that is not fatal, and the sweep's thread
/// ended with nothing latched.
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
    let (held, swept) = lapsed.recv_timeout(GATE_WAIT).expect("the lapse");
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
    fixture.assert_runtime_runs();
    assert!(
        fixture.host.sweep_thread_runs_for_test(),
        "the lease sweep's thread still runs"
    );
    // The fence and X's held startup claim stay as the self-check left them; the Host close of
    // them is not this test's subject.
    fixture.close_unchecked();
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
        .wait_for_grant(GATE_WAIT)
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
/// (register §1.364).
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

// ---- Workflow #461: the lapsed lease met before the sweep, by a disconnect and by a failed input.

/// The owner's resource disposition that each confirmed close of `instance` in `events` recorded.
fn close_owner_dispositions(
    events: &[PersistedEvent],
    instance: InstanceId,
) -> Vec<OwnerResourceDisposition> {
    phases(events)
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id,
                quiescence: ResourceQuiescence::Confirmed,
                owner_disposition,
                ..
            } if instance_id == instance => Some(owner_disposition),
            _ => None,
        })
        .collect()
}

/// `failure` is the failure the test-only switch injects into the record of a refused close:
/// `ledger_failure` at `fence_instance_resources`, fatal and Global.
fn assert_record_failure(failure: &RuntimeHostError) {
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
}

impl Fixture {
    /// The Runtime clock moved just past `lease`'s time, and the business capacity sampled then.
    fn lapse_past(&self, lease: &LeaseToken) {
        self.clock.advance(
            lease
                .expires_at_monotonic_ms()
                .saturating_sub(self.clock.monotonic_ms())
                + 1,
        );
        self.sample_capacity();
    }

    /// The next input of `alias` lets `lease` lapse while its write is in flight (the Runtime clock
    /// moved just past the lease's time, as a device call longer than the lease moves it), and
    /// then fails with its resources confirmed (`fail_input`).
    fn lapse_inside_the_next_input(&self, alias: &str, lease: &LeaseToken) {
        let clock = Arc::clone(&self.clock);
        let lapses_at = lease.expires_at_monotonic_ms();
        let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
            clock.advance(lapses_at.saturating_sub(clock.monotonic_ms()) + 1);
        });
        *self.states[alias].input_hook.lock().expect("input hook") = Some(hook);
        self.states[alias].fail_input.store(true, Ordering::Release);
    }

    /// A safe reset under `lease`, sent on `connection`: its request and what the host answered.
    fn reset_input(
        &self,
        lease: &LeaseToken,
        connection: ConnectionId,
    ) -> (RuntimeRequest, RuntimeHostResult<RuntimeReceipt>) {
        let request = self.request(RuntimeOperation::Input {
            frame: None,
            token: lease.clone(),
            action: InputAction::Reset,
        });
        let answered = self.host.process_request_for_test(&request, connection);
        (request, answered)
    }

    /// X alone is fenced, with `lease_expired` as its cause (one fence record after `after`, which
    /// names X), and the refusal of X's close under `lease` is recorded once after `after`, naming
    /// X, the lease and the stage `runtime_lifecycle_session_close`, under `request` when given.
    fn assert_x_fenced_by_the_refusal(
        &self,
        after: u64,
        lease: &LeaseToken,
        request: Option<&RuntimeRequest>,
    ) {
        let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
        self.assert_only_fenced(X, lease_expired, after);
        let events = self.events_after(after);
        let refusals = failures(&events, lease_expired);
        let [refusal] = refusals.as_slice() else {
            panic!("one record of the refusal: {refusals:#?}");
        };
        assert_eq!(refusal.links().instance_id(), Some(&self.ids[X]));
        if let Some(request) = request {
            assert_eq!(refusal.links().request_id(), Some(&request.request_id()));
        }
        let outcome = failure_outcome(refusal);
        assert_eq!(
            outcome.value("lease_id"),
            Some(&serde_json::to_value(lease.lease_id()).expect("lease id")),
            "{outcome:?}"
        );
        assert_eq!(
            outcome.value("stage").and_then(Value::as_str),
            Some(HostLocation::RuntimeLifecycleSessionClose.as_str()),
            "{outcome:?}"
        );
    }

    /// After `after`, X's lapsed `lease` was ended once by its holder's own end (`lease.released`)
    /// and never by the sweep (no `lease.expired`), and X's session, `closes_before` capture
    /// closes before, was closed once after that end, under a fenced resource-close step.
    fn assert_closed_after_the_holders_end(
        &self,
        after: u64,
        lease: &LeaseToken,
        closes_before: usize,
    ) {
        let events = self.events_after(after);
        assert!(
            lease_records(&events, EventType::LeaseExpired, lease.lease_id()).is_empty(),
            "the holder's own end ended X's lease, never the sweep"
        );
        let releases = lease_records(&events, EventType::LeaseReleased, lease.lease_id());
        let [release] = releases.as_slice() else {
            panic!("X's lease is released once: {releases:#?}");
        };
        let closes = confirmed_closes(&events, self.ids[X]);
        assert!(
            closes.len() == 1 && closes[0] > release.sequence(),
            "X's session is closed once, after its lease ended: {closes:?}"
        );
        assert_eq!(
            self.states[X].capture_close_count.load(Ordering::Acquire),
            closes_before + 1,
            "X's session is closed"
        );
        assert_eq!(
            self.states[X]
                .unfenced_capture_close_count
                .load(Ordering::Acquire),
            0,
            "under a fenced resource-close step"
        );
    }

    /// Y is not fenced, and a direct run of Y is dispatched and completes, the Runtime still
    /// running.
    fn assert_y_runs(&self) {
        assert_eq!(status_fence(&self.host, Y), None, "Y is not fenced");
        self.sample_capacity();
        let package = self.run_package(Y);
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let run = self.request(RuntimeOperation::run_contained_task(
            Y,
            issuer.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ));
        let receipt = self
            .host
            .process_request_for_test(&run, self.connection())
            .expect("Y's run's receipt");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            ),
            "{receipt:?}"
        );
        self.assert_runtime_runs();
    }
}

/// #461, the first path: X's observation opens X's session (X on the multi-Nemu gate, so every
/// lease end of X closes it) and a client's lease of X lapses with it open while the sweep's
/// expiry scan is held; then the client's connection closes, and its end (as the connection's
/// thread runs it once its stream is gone) ends the lease as a disconnect, whose close under the
/// lapsed lease is refused `lease_expired`. The connection's end returns no error and the
/// Runtime's health stays clean; X alone is fenced, with `lease_expired` as its cause; the refusal
/// is recorded once, naming X, X's lease and the stage `runtime_lifecycle_session_close`; X's lease
/// is released once by the disconnect and never expired, and X's session is closed once after
/// that, under a fenced close; then Y, another instance, is dispatched a direct run, which
/// completes. Before the fix the refusal was fatal and the connection's end latched it.
#[test]
fn close461_a_disconnect_after_its_lease_lapsed_fences_only_its_instance_and_the_runtime_runs() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    let x_closes = fixture.states[X]
        .capture_close_count
        .load(Ordering::Acquire);
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();
    fixture.lapse_past(&x_lease);

    let ended = fixture.host.end_connection_for_test(connection);

    assert!(ended.is_ok(), "the connection's end: {ended:?}");
    fixture.assert_runtime_runs();
    fixture.assert_x_fenced_by_the_refusal(mark, &x_lease, None);
    fixture.assert_closed_after_the_holders_end(mark, &x_lease, x_closes);
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}

/// #461, the second path: X's observation opens X's session and a client's lease of X holds X;
/// a safe reset under that lease lets the lease lapse while its write is in flight and then fails
/// (its resources confirmed), the sweep's expiry scan held. The input's clean-up closes the
/// session under the lapsed lease, which refuses it `lease_expired`, and the input's own lease end
/// (a backend failure's) meets the same refusal. The input fails with its own failure
/// (`backend_operation_failed`) and the Runtime's health stays clean; X alone is fenced, with
/// `lease_expired` as its cause; the refusal is recorded once, under the input's request, naming
/// X, X's lease and the stage `runtime_lifecycle_session_close`; X's lease is released once by the
/// input's own end and never expired, and X's session is closed once after that, under a fenced
/// close, whose record keeps no owner unconfirmed; then Y is dispatched a direct run, which
/// completes, and the host closes, which an owner kept unconfirmed would refuse. Before the fix the
/// refusal was fatal and the owner was kept unconfirmed.
#[test]
fn close461_an_input_whose_lease_lapsed_under_it_fails_alone_and_keeps_no_owner_unconfirmed() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    let x_closes = fixture.states[X]
        .capture_close_count
        .load(Ordering::Acquire);
    fixture.lapse_inside_the_next_input(X, &x_lease);
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let (input, answered) = fixture.reset_input(&x_lease, connection);

    let failed = answered.expect("the input's receipt");
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.error_projection().expect("the input's failure").code,
        RuntimeErrorCode::BackendOperationFailed,
        "{failed:?}"
    );
    fixture.assert_runtime_runs();
    fixture.assert_x_fenced_by_the_refusal(mark, &x_lease, Some(&input));
    fixture.assert_closed_after_the_holders_end(mark, &x_lease, x_closes);
    let dispositions = close_owner_dispositions(&fixture.events_after(mark), fixture.ids[X]);
    assert!(
        !dispositions.is_empty() && !dispositions.contains(&OwnerResourceDisposition::Unconfirmed),
        "the close keeps no owner unconfirmed: {dispositions:?}"
    );
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}

/// #461, a failure of the Runtime itself on the first path stays fatal: X's client lease lapses
/// with X's session open, the record of the refused close that would fence X cannot be appended
/// (the ledger's failure, injected by #458's test-only switch, which holds until the host ends),
/// and the client's connection closes. The connection's end returns that failure, `ledger_failure`
/// at `fence_instance_resources`, fatal and Global, and the Runtime holds it latched; no instance
/// is fenced.
#[test]
fn close461_a_record_that_cannot_be_appended_at_a_disconnects_close_still_stops_the_runtime() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    fixture.lapse_past(&x_lease);

    let ended = fixture.host.end_connection_for_test(connection);

    assert_record_failure(&ended.expect_err("the connection's end returns the record's failure"));
    let latched = fixture
        .host
        .fatal_error()
        .expect("runtime health")
        .expect("the Runtime latched the record's failure");
    assert_record_failure(&latched);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    drop(scan);
    fixture.close_unchecked();
}

/// #461, a failure of the Runtime itself on the second path stays fatal: a safe reset of X lets
/// its client lease lapse while its write is in flight and then fails, and the record of the
/// refused close that would fence X cannot be appended (#458's test-only switch). The Runtime
/// holds that failure latched, `ledger_failure` at `fence_instance_resources`, fatal and Global;
/// no instance is fenced. Workflow #475 fix round 2: and the owner is not retained as unconfirmed.
/// The failure is the ledger's own, which no record carries (`append_lifecycle_failure` hands a
/// ledger failure back), so the failed input's clean-up (`finish_input_failure`'s `Err` arm)
/// returns it at its record, before the owner's retention it makes for a failure it can record
/// (the refusal itself, before #461), and the request's boundary latches it (`contain_failure`),
/// which retains nothing. The refused close began nothing, so no device step is left half done.
#[test]
fn close461_a_record_that_cannot_be_appended_at_a_failed_inputs_close_still_stops_the_runtime() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    fixture.lapse_inside_the_next_input(X, &x_lease);
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let _answered = fixture.reset_input(&x_lease, connection);

    let latched = fixture
        .host
        .fatal_error()
        .expect("runtime health")
        .expect("the Runtime latched the record's failure");
    assert_record_failure(&latched);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    assert!(
        !fixture
            .host
            .owner_retained_unconfirmed_for_test()
            .expect("the owner's disposition"),
        "the ledger's own failure is latched at the request's boundary, which retains no owner"
    );
    drop(scan);
    fixture.close_unchecked();
}

// ---- Workflow #475: five more closes under a key whose lease lapsed before the sweep met it.

/// The policy instance whose run hands its key to its ladder (the fixture catalog's instance).
const RUN: &str = POLICY_INSTANCE_ALIAS;
/// The backends (touch, capture) X and Y register in the backend switch's fixture.
const SWITCH_FROM: (&str, &str) = ("adb_shell_input", "adb");
/// The backends the switch of X asks for.
const SWITCH_TO: (&str, &str) = ("minitouch", "droidcast_raw");

/// `lease_id` as a record's outcome names it.
fn lease_value(lease_id: &LeaseId) -> Value {
    serde_json::to_value(lease_id).expect("a lease id encodes")
}

/// The records of `event_type` in `events` linked to the lease `lease` names.
fn lease_events<'a>(
    events: &'a [PersistedEvent],
    event_type: EventType,
    lease: &Value,
) -> Vec<&'a PersistedEvent> {
    events
        .iter()
        .filter(|event| {
            event.event_type() == event_type
                && event
                    .links()
                    .lease_id()
                    .is_some_and(|lease_id| lease_value(lease_id) == *lease)
        })
        .collect()
}

/// The transfers in `events` that handed on the lease `lease` names.
fn handed_on_from<'a>(events: &'a [PersistedEvent], lease: &Value) -> Vec<&'a PersistedEvent> {
    events
        .iter()
        .filter(|event| match event.payload() {
            EventPayload::Lease(LeasePayload::Transferred(transfer)) => {
                lease_value(&transfer.from_lease_id()) == *lease
            }
            _ => false,
        })
        .collect()
}

/// The refused session closes in `events` (`lease_expired` at the stage
/// `runtime_lifecycle_session_close`) that name `instance`.
fn session_close_refusals(events: &[PersistedEvent], instance: InstanceId) -> Vec<&PersistedEvent> {
    failures(events, RuntimeErrorCode::LeaseExpired.as_str())
        .into_iter()
        .filter(|event| {
            event.links().instance_id() == Some(&instance)
                && failure_outcome(event)
                    .value("stage")
                    .and_then(Value::as_str)
                    == Some(HostLocation::RuntimeLifecycleSessionClose.as_str())
        })
        .collect()
}

/// `instance`'s id as the configuration's keys spell it.
fn id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The file layer's two backend rows of `instance` (`SWITCH_FROM`, explicit, as actingd reports
/// them), annotated by the configuration's key table (`gate_hs2.rs`'s manifest).
fn switch_manifest(instance: InstanceId) -> RuntimeConfigManifest {
    let mut parameters = [
        (
            instance_key(&id_text(instance), TOUCH_BACKEND),
            SWITCH_FROM.0,
        ),
        (
            instance_key(&id_text(instance), CAPTURE_BACKEND),
            SWITCH_FROM.1,
        ),
    ]
    .into_iter()
    .map(|(key, value)| ConfigParameter {
        key,
        value: FactScalar::String(value.to_owned()),
        source: ConfigParameterSource::Explicit,
        settable: false,
    })
    .collect::<Vec<_>>();
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

impl Fixture {
    /// The members A and B of one device group of several, writing one session log, and Y alone
    /// on its own device with no device configuration.
    fn group_beside_y() -> Self {
        let root = TempDir::new().expect("tempdir");
        let ids = BTreeMap::from([(A, instance_id()), (B, instance_id()), (Y, instance_id())]);
        let device = ids[A].min(ids[B]);
        let states = [A, B, Y]
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
        *states[A]
            .application_id
            .lock()
            .expect("fake application id") = Some("neutral.member.a".to_owned());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(stuck_recovery_off(&[A, B, Y]))
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
            next_connection: AtomicU64::new(4_750),
            root,
        }
    }

    /// X and Y in #402 HS-2's shape (`gate_hs2.rs`): each alone on its device with a bound ADB
    /// endpoint (the start prepares each and keeps its session), X's fake registered with the
    /// backends `SWITCH_FROM` and the file layer's rows of X's two backend keys, on a manual
    /// Runtime clock, stuck recovery off.
    fn switching() -> Self {
        let root = TempDir::new().expect("tempdir");
        let ids = BTreeMap::from([(X, instance_id()), (Y, instance_id())]);
        let states = [X, Y]
            .into_iter()
            .map(|alias| {
                let state = Arc::new(FakeState::default());
                state.observed_open.store(true, Ordering::Release);
                (alias, state)
            })
            .collect::<BTreeMap<_, _>>();
        *states[X].backends.lock().expect("fake backends") =
            Some((SWITCH_FROM.0.to_owned(), SWITCH_FROM.1.to_owned()));
        let provider = FakeProvider::from_entries(
            states
                .iter()
                .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
                .collect::<Vec<_>>(),
        )
        .with_device_endpoints();
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(stuck_recovery_off(&[X, Y]))
            .with_config_manifest(switch_manifest(ids[X]));
        let host = RuntimeHost::start(config, Arc::new(provider)).expect("start the switch's host");
        Self {
            host: Arc::new(host),
            clock,
            ids,
            states,
            log: DeviceSessionLog::default(),
            next_connection: AtomicU64::new(4_751),
            root,
        }
    }

    /// `RUN`, the policy instance, and Y, both on the Nemu IPC (so each closes its session at
    /// every lease end), with the ladder fixture's configuration (`ladder_setup`: stuck recovery as
    /// shipped, the scheduled package as `RUN`'s startup package), on a manual Runtime clock; the
    /// policy's configured identities are the two aliases, only `RUN` available to it. `RUN`'s
    /// scheduled run, which binds a return-home package, is admitted: its context and request.
    fn policy_run() -> (Self, Box<PolicyRunContext>, ContainedTaskRequest) {
        let root = TempDir::new().expect("tempdir");
        let (config, request) = ladder_setup(root.path());
        let ids = BTreeMap::from([(RUN, instance_id()), (Y, instance_id())]);
        let states = [RUN, Y]
            .into_iter()
            .map(|alias| (alias, Arc::new(FakeState::default())))
            .collect::<BTreeMap<_, _>>();
        states[RUN]
            .physical_task_geometry
            .store(true, Ordering::Release);
        let provider = FakeProvider::from_entries(
            states
                .iter()
                .map(|(alias, state)| ((*alias).to_owned(), ids[alias], Arc::clone(state)))
                .collect::<Vec<_>>(),
        );
        let run = provider
            .resolve(RUN)
            .expect("the policy instance")
            .with_configuration(nemu_capture("TESTSERIAL0001"));
        let y = provider
            .resolve(Y)
            .expect("Y")
            .with_configuration(nemu_capture("TESTSERIAL0002"));
        let provider = provider.with_resolved_for(RUN, run).with_resolved_for(Y, y);
        let mut facts = policy_facts();
        let mut other = facts.instances[0].clone();
        other.instance_id = Y.to_owned();
        other.available = false;
        facts.instances.push(other);
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = RuntimeHost::start(
            config
                .with_runtime_clock(clock.clone())
                .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources())),
            Arc::new(provider),
        )
        .expect("start the policy host");
        let fixture = Self {
            host: Arc::new(host),
            clock,
            ids,
            states,
            log: DeviceSessionLog::default(),
            next_connection: AtomicU64::new(4_752),
            root,
        };
        fixture.sample_capacity();
        fixture
            .host
            .activate_policy_catalog(&policy_sources(1))
            .expect("activate the policy catalog");
        let cycle = fixture
            .host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .expect("evaluate the policy");
        let evaluation = cycle.evaluation.as_ref().expect("a policy evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.instance_id == RUN)
            .expect("the policy instance's dispatch intent")
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the intent's reason chain")
            .clone();
        record_policy_approval(&fixture.host, &intent);
        let PolicyDispatchAdmission::Granted { context } = fixture
            .host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&fixture.host, &intent),
                &request,
            )
            .expect("the policy admission")
        else {
            panic!("the policy instance's run is granted")
        };
        (fixture, context, request)
    }

    /// The next close of an instance's resources that finds a session to close lets every lease
    /// the test holds lapse right before it asks its write authority (the Runtime clock moved past
    /// them), as a device call that outlasts the lease lets it lapse there.
    fn lapse_inside_the_next_close(&self) {
        let clock = Arc::clone(&self.clock);
        self.host
            .run_before_next_resource_close_for_test(Box::new(move || clock.advance(LAPSE_MS)))
            .expect("arm the lapse");
    }

    /// Fix round 1: as [`Self::lapse_inside_the_next_close`], one close later. The next close of
    /// an instance's resources that finds a session to close runs as it would, and arms the lapse
    /// for the close after it.
    fn lapse_inside_the_close_after_the_next(&self) {
        let host = Arc::clone(&self.host);
        let clock = Arc::clone(&self.clock);
        self.host
            .run_before_next_resource_close_for_test(Box::new(move || {
                host.run_before_next_resource_close_for_test(Box::new(move || {
                    clock.advance(LAPSE_MS);
                }))
                .expect("arm the lapse in the close after it");
            }))
            .expect("arm the next close");
    }

    /// Only `alias` is fenced, with `lease_expired` as its cause (in the status and in one fence
    /// record after `after`, which names it), and the refusal of its session's close is recorded
    /// once after `after`, at the stage `runtime_lifecycle_session_close`, naming it: that record.
    fn assert_fenced_by_one_refusal(&self, alias: &str, after: u64) -> PersistedEvent {
        self.assert_only_fenced(alias, RuntimeErrorCode::LeaseExpired.as_str(), after);
        let events = self.events_after(after);
        let refusals = session_close_refusals(&events, self.ids[alias]);
        let [refusal] = refusals.as_slice() else {
            panic!("one record of the refused close: {refusals:#?}");
        };
        (*refusal).clone()
    }

    /// After `after`, the lease `lease` names was released once, never expired and never handed
    /// on, and `alias`'s session was closed once, after that release: the close's sequence.
    fn assert_released_then_closed_once(&self, alias: &str, after: u64, lease: &Value) -> u64 {
        let events = self.events_after(after);
        assert!(
            lease_events(&events, EventType::LeaseExpired, lease).is_empty(),
            "the lease is ended by its holder, never by the sweep"
        );
        assert!(
            handed_on_from(&events, lease).is_empty(),
            "the lease is handed to no one"
        );
        let releases = lease_events(&events, EventType::LeaseReleased, lease);
        let [release] = releases.as_slice() else {
            panic!("the lease is released once: {releases:#?}");
        };
        let closes = confirmed_closes(&events, self.ids[alias]);
        assert!(
            closes.len() == 1 && closes[0] > release.sequence(),
            "{alias}'s session is closed once, after the lease ended: {closes:?}"
        );
        closes[0]
    }

    /// A client's claim of `alias` at high priority, which asks for the pre-emption of a lease
    /// of normal priority that holds it: the request and what the host answered.
    fn preempt(&self, alias: &str) -> (RuntimeRequest, RuntimeHostResult<RuntimeReceipt>) {
        let holder = IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder");
        let request = self.request(RuntimeOperation::queue_lease(
            alias,
            holder,
            LeaseQueuePolicy::new(LeasePriority::High, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS)
                .expect("queue policy"),
        ));
        let answered = self
            .host
            .process_request_for_test(&request, self.connection());
        (request, answered)
    }

    /// A client's `launch` of `alias`'s application, on the command's own claim: the request and
    /// what the host answered.
    fn launch(&self, alias: &str) -> (RuntimeRequest, RuntimeHostResult<RuntimeReceipt>) {
        let holder = IdentifierIssuer::new()
            .expect("issuer")
            .mint_holder_id()
            .expect("holder");
        let request = self.request(RuntimeOperation::ApplicationLifecycle {
            instance_alias: alias.to_owned(),
            holder_id: *holder.transport(),
            action: ApplicationLifecycleAction::Launch,
        });
        let answered = self
            .host
            .process_request_for_test(&request, self.connection());
        (request, answered)
    }

    /// The switch of `alias`'s backends to `SWITCH_TO`, as `actingctl config set` sends it: the
    /// request and what the host answered.
    fn switch_backends(&self, alias: &str) -> (RuntimeRequest, RuntimeHostResult<RuntimeReceipt>) {
        let id = id_text(self.ids[alias]);
        let change = |key: String, value: &str| ConfigSettingChange {
            key,
            value: Some(json!(value)),
        };
        let request = self.request(RuntimeOperation::ConfigureSettings {
            changes: vec![
                change(instance_key(&id, TOUCH_BACKEND), SWITCH_TO.0),
                change(instance_key(&id, CAPTURE_BACKEND), SWITCH_TO.1),
            ],
            expected_version: None,
        });
        let answered = self
            .host
            .process_request_for_test(&request, self.connection());
        (request, answered)
    }

    /// The Runtime holds the failure the test-only switch injects into the record of a refused
    /// close latched (`ledger_failure` at `fence_instance_resources`, fatal and Global), and no
    /// instance is fenced.
    fn assert_record_failure_latched(&self) {
        let latched = self
            .host
            .fatal_error()
            .expect("runtime health")
            .expect("the Runtime latched the record's failure");
        assert_record_failure(&latched);
        assert!(fence_records(&all_events(&self.host)).is_empty());
    }
}

/// #475, the first place: a failed run's hand-over of its key to its ladder. `RUN`'s scheduled
/// run binds a return-home package; its second capture fails (`capture_backend_operation_failed`,
/// which the shipped trigger item `task_capture_backend` takes), and the run's lease lapses inside
/// the close of that capture failure (the test's hook), before the run's end began, so the run has
/// no end phase and #411 A64 does not take the lapse: that close is refused and fences `RUN`
/// (#456). The run's failed end admits a ladder and hands it the key; the hand-over's close meets
/// the lapsed lease too. The run returns its own failure, nonfatal; the Runtime's health stays
/// clean; up to the run lease's end only `RUN` is fenced, by one fence record with
/// `lease_expired`; the hand-over's refusal is recorded once after the run's terminal, naming
/// `RUN`, the run's lease and the stage `runtime_lifecycle_session_close`; the run's lease is
/// released once, never expired and never handed on; `RUN`'s session is closed after that
/// release, and only after that close is the ladder's claim queued (high priority); Y then runs a
/// direct run to its end. Before the fix the hand-over's refusal stopped the Runtime. Fix round 1:
/// the hand-over itself (`hand_off_to_ladder`) withheld the key once, which a ladder that waits in
/// the queue instead never reaches (the records of the two are the same).
#[test]
fn close475_a_failed_runs_hand_over_to_its_ladder_after_its_lease_lapsed_fences_only_its_instance()
{
    let (fixture, context, request) = Fixture::policy_run();
    let lease = lease_value(&context.lease_token().lease_id());
    fixture.states[RUN]
        .fail_capture_on
        .store(2, Ordering::Release);
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let error = fixture
        .host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the run fails");

    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(
        error.code(),
        SharedCode::CaptureBackendOperationFailed.as_str(),
        "{error:?}"
    );
    fixture.assert_runtime_runs();
    let events = fixture.events_after(mark);
    let run_id = context.run_id();
    let terminal = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskFailed && event.links().run_id() == Some(&run_id)
        })
        .map(PersistedEvent::sequence)
        .expect("the run's task.failed");
    let releases = lease_events(&events, EventType::LeaseReleased, &lease);
    let [released] = releases.as_slice() else {
        panic!("the run's lease is released once: {releases:#?}");
    };
    let release = released.sequence();
    assert!(
        lease_events(&events, EventType::LeaseExpired, &lease).is_empty(),
        "the run's own end ended its lease, never the sweep"
    );
    assert!(
        handed_on_from(&events, &lease).is_empty(),
        "the run's key is handed to no one"
    );
    assert_eq!(
        fixture
            .host
            .ladder_hand_offs_withheld_for_test()
            .expect("the hand-overs withheld"),
        1,
        "the run's failed end took the hand-over to its ladder, which withheld the key for the \
         owed close (a ladder waiting in the queue never reaches the hand-over)"
    );
    let until_release = events
        .iter()
        .filter(|event| event.sequence() <= release)
        .cloned()
        .collect::<Vec<_>>();
    let fences = fence_records(&until_release);
    let [(fence, fence_outcome)] = fences.as_slice() else {
        panic!("one fence record up to the run lease's end: {fences:#?}");
    };
    assert_eq!(fence.links().instance_id(), Some(&fixture.ids[RUN]));
    assert_fence(
        fence_outcome,
        fixture.ids[RUN],
        RuntimeErrorCode::LeaseExpired.as_str(),
    );
    let ended = events
        .iter()
        .filter(|event| event.sequence() > terminal)
        .cloned()
        .collect::<Vec<_>>();
    let refusals = session_close_refusals(&ended, fixture.ids[RUN]);
    let [refusal] = refusals.as_slice() else {
        panic!("the hand-over's refusal is recorded once: {refusals:#?}");
    };
    assert!(
        refusal.sequence() < release,
        "refused before the lease ended"
    );
    assert_eq!(
        failure_outcome(refusal).value("lease_id"),
        Some(&lease),
        "the refusal names the run's lease"
    );
    let close = confirmed_closes(&events, fixture.ids[RUN])
        .into_iter()
        .find(|sequence| *sequence > release)
        .expect("the run's instance's session is closed after its lease ended");
    let ladder_queued = events.iter().find(|event| {
        event.links().instance_id() == Some(&fixture.ids[RUN])
            && matches!(
                event.payload(),
                EventPayload::Scheduler(SchedulerPayload::Queued(queued))
                    if queued.priority() == LeasePriority::High
            )
    });
    assert!(
        ladder_queued.is_some_and(|queued| queued.sequence() > close),
        "the ladder's claim is queued only after the close: {ladder_queued:?}"
    );
    drop(scan);
    fixture.assert_y_runs();
    // The ladder climbs on its own and the fence stays: the Host close of them is not this test's
    // subject.
    fixture.close_unchecked();
}

/// #475, the first place, a failure of the Runtime itself stays fatal: as above, but once the run's
/// terminal is written (its lease end's checkpoint) the record of a refused close cannot be
/// appended (#458's test-only switch). The hand-over's refusal then fails with the record's
/// failure, `ledger_failure` at `fence_instance_resources`, fatal and Global, which the run returns
/// or the Runtime holds latched; nothing is fenced after the run's terminal (the capture failure's
/// own close fenced `RUN` before it, as #456 does).
#[test]
fn close475_a_record_that_cannot_be_appended_at_a_runs_hand_over_to_its_ladder_still_stops_the_runtime()
 {
    let (fixture, context, request) = Fixture::policy_run();
    fixture.states[RUN]
        .fail_capture_on
        .store(2, Ordering::Release);
    fixture.lapse_inside_the_next_close();
    let host = Arc::clone(&fixture.host);
    let lease_end = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::FailedRunLeaseEnd,
            fixture.ids[RUN],
            context.lease_token().lease_id(),
            move |_| {
                host.fail_refused_close_record_for_test(true)
                    .expect("the record's failure");
            },
        )
        .expect("arm the run's lease end");
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let returned = fixture
        .host
        .run_scheduled_contained_task(&context, &request);

    assert_eq!(lease_end.consumed(), 1, "the run reached its lease end");
    let failure = match fixture.host.fatal_error().expect("runtime health") {
        Some(latched) => latched,
        None => returned.expect_err("the run returns the record's failure"),
    };
    assert_record_failure(&failure);
    let events = fixture.events_after(mark);
    let run_id = context.run_id();
    let terminal = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskFailed && event.links().run_id() == Some(&run_id)
        })
        .map(PersistedEvent::sequence)
        .expect("the run's task.failed");
    let ended = events
        .iter()
        .filter(|event| event.sequence() > terminal)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        fence_records(&ended).is_empty(),
        "nothing is fenced at the hand-over"
    );
    drop(scan);
    fixture.close_unchecked();
}

/// #475, the second place: a pre-emption's hand-over. X's observation opens X's session and a
/// client's lease of X holds X; another client's claim of X at high priority asks for its
/// pre-emption, and the idle lease is handed over at once (`promote_idle_preemption`); the lease
/// lapses inside that hand-over's close (the test's hook). The claim answers `queued`: nothing
/// was handed to it. The Runtime's health stays clean; X alone is fenced, with `lease_expired` as
/// its cause; the refusal is recorded once, naming X, X's lease and the stage
/// `runtime_lifecycle_session_close`; X's lease is released once (the pre-emption's end), never
/// expired and never handed on; X's session is closed once after that, under a fenced close, and
/// no grant of the claim precedes that close; Y then runs a direct run to its end. Before the fix
/// the refusal stopped the Runtime.
#[test]
fn close475_a_preemptions_hand_over_of_a_lapsed_lease_fences_only_its_instance() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let x_lease = fixture.acquire_on(X, fixture.connection());
    fixture.sample_capacity();
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let (claim, answered) = fixture.preempt(X);

    let answered = answered.expect("the claim's receipt");
    assert_eq!(
        answered.state(),
        RuntimeReceiptState::Queued,
        "{answered:?}"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    let lease = lease_value(&x_lease.lease_id());
    assert_eq!(failure_outcome(&refusal).value("lease_id"), Some(&lease));
    let close = fixture.assert_released_then_closed_once(X, mark, &lease);
    assert_eq!(
        fixture.states[X]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0,
        "under a fenced resource-close step"
    );
    let events = fixture.events_after(mark);
    assert!(
        events
            .iter()
            .filter(|event| event.links().request_id() == Some(&claim.request_id()))
            .all(|event| event.event_type() != EventType::LeaseTransferred
                && (event.event_type() != EventType::LeaseGranted || event.sequence() > close)),
        "the claim is handed nothing, and no grant of it precedes X's close"
    );
    drop(scan);
    fixture.assert_y_runs();
    // The claim may still wait behind X's fence: the Host close of it is not this test's subject.
    fixture.close_unchecked();
}

/// #475, the second place, a failure of the Runtime itself stays fatal: as above, with the record
/// of the refused close made to fail (#458's test-only switch). The Runtime holds `ledger_failure`
/// at `fence_instance_resources` latched, fatal and Global; no instance is fenced.
#[test]
fn close475_a_record_that_cannot_be_appended_at_a_preemptions_close_still_stops_the_runtime() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let _held = fixture.acquire_on(X, fixture.connection());
    fixture.sample_capacity();
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let _answered = fixture.preempt(X);

    fixture.assert_record_failure_latched();
    drop(scan);
    fixture.close_unchecked();
}

/// #475 fix round 1, the second place at an input's end (`transfer_preempted_while_guarded`): X's
/// observation opens X's session and a client's lease of X holds X; a safe reset under that lease
/// is under way when another client's claim of X at high priority asks for the lease's
/// pre-emption, which waits for the reset's write (the claim answers `queued`, its pre-emption
/// requested); the write returns, the reset's end hands the lease over, and the lease lapses
/// inside that hand-over's close of X's session (the test's hook). The reset completes; the
/// Runtime's health stays clean; X alone is fenced, with `lease_expired` as its cause; the refusal
/// is recorded once, naming X, X's lease and the stage `runtime_lifecycle_session_close`; X's
/// lease is released once (the pre-emption's end), never expired and never handed on; X's session
/// is closed once after that, under a fenced close, and no grant of the claim precedes that close;
/// Y then runs a direct run to its end. Before the fix the refusal stopped the Runtime.
#[test]
fn close475_a_preemption_at_an_inputs_end_whose_lease_lapsed_fences_only_its_instance() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    fixture.sample_capacity();
    // The reset's write reports `true` once it is in flight, the reset `false` once it ended; the
    // write returns when the test lets it.
    let (report, reports) = mpsc::channel::<bool>();
    let (let_write_return, write_returns) = mpsc::channel::<()>();
    let write_report = report.clone();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        write_report
            .send(true)
            .expect("the reset's write is in flight");
        write_returns.recv().expect("the reset's write returns");
    });
    *fixture.states[X].input_hook.lock().expect("input hook") = Some(hook);
    let reset = fixture.request(RuntimeOperation::Input {
        frame: None,
        token: x_lease.clone(),
        action: InputAction::Reset,
    });
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    // The scope owns the channel's ends, so a failed step drops them and the reset's write returns.
    let (fixture_ref, host) = (&fixture, &fixture.host);
    let ((claim, queued), answered) = thread::scope(move |scope| {
        let input = scope.spawn(move || {
            let answered = host.process_request_for_test(&reset, connection);
            report.send(false).expect("the reset ended");
            answered
        });
        assert!(
            reports.recv().expect("the reset reports"),
            "the reset ended before its write"
        );
        let preempted = fixture_ref.preempt(X);
        fixture_ref.lapse_inside_the_next_close();
        let_write_return
            .send(())
            .expect("let the reset's write return");
        (preempted, input.join().expect("the reset's thread"))
    });

    let queued = queued.expect("the claim's receipt");
    assert_eq!(queued.state(), RuntimeReceiptState::Queued, "{queued:?}");
    assert!(
        matches!(
            queued.result(),
            Some(RuntimeResult::LeaseQueued { status }) if status.preempt_requested()
        ),
        "the claim asks for the pre-emption, which waits for the write: {queued:?}"
    );
    let answered = answered.expect("the reset's receipt");
    assert_eq!(
        answered.state(),
        RuntimeReceiptState::Completed,
        "{answered:?}"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    let lease = lease_value(&x_lease.lease_id());
    assert_eq!(failure_outcome(&refusal).value("lease_id"), Some(&lease));
    let close = fixture.assert_released_then_closed_once(X, mark, &lease);
    assert_eq!(
        fixture.states[X]
            .unfenced_capture_close_count
            .load(Ordering::Acquire),
        0,
        "under a fenced resource-close step"
    );
    let events = fixture.events_after(mark);
    assert!(
        events
            .iter()
            .filter(|event| event.links().request_id() == Some(&claim.request_id()))
            .all(|event| event.event_type() != EventType::LeaseTransferred
                && (event.event_type() != EventType::LeaseGranted || event.sequence() > close)),
        "the claim is handed nothing, and no grant of it precedes X's close"
    );
    drop(scan);
    fixture.assert_y_runs();
    // The claim may still wait behind X's fence: the Host close of it is not this test's subject.
    fixture.close_unchecked();
}

/// #475, the third place: an application command's preparation. X's observation opens X's
/// session; a client's `launch` of X's application takes the command's own claim, and the claim
/// lapses inside the preparation's close of X's session (the test's hook), before any command is
/// sent. The command fails with the refusal as its own failure (`lease_expired`, not fatal) and
/// no command reaches X's device; the Runtime's health stays clean; X alone is fenced, with
/// `lease_expired` as its cause; the refusal is recorded once, under the command's request,
/// naming X, the claim's lease and the stage `runtime_lifecycle_session_close`; the claim's lease
/// is released once (the command's clean-up), never expired and never handed on; X's session is
/// closed once after that; Y then runs a direct run to its end, and the host closes. Before the fix
/// the refusal stopped the Runtime.
#[test]
fn close475_an_application_command_whose_key_lapsed_before_its_preparation_fails_alone() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let (command, answered) = fixture.launch(X);

    let failed = answered.expect("the command's receipt");
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let projection = failed.error_projection().expect("the command's failure");
    assert!(!projection.fatal, "{failed:?}");
    assert_eq!(
        projection.code,
        RuntimeErrorCode::LeaseExpired,
        "{failed:?}"
    );
    assert!(
        fixture.states[X]
            .application_actions
            .lock()
            .expect("fake application actions")
            .is_empty(),
        "no command reached X's device"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    assert_eq!(refusal.links().request_id(), Some(&command.request_id()));
    let lease = failure_outcome(&refusal)
        .value("lease_id")
        .cloned()
        .expect("the refusal names the command's lease");
    fixture.assert_released_then_closed_once(X, mark, &lease);
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}

/// #475, the third place, a failure of the Runtime itself stays fatal: as above, with the record
/// of the refused close made to fail (#458's test-only switch). The Runtime holds `ledger_failure`
/// at `fence_instance_resources` latched, fatal and Global; no instance is fenced.
#[test]
fn close475_a_record_that_cannot_be_appended_at_an_application_preparation_still_stops_the_runtime()
{
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let _answered = fixture.launch(X);

    fixture.assert_record_failure_latched();
    drop(scan);
    fixture.close_unchecked();
}

/// #475, the fourth place: the backend switch. X and Y in #402 HS-2's shape; X's observation keeps
/// X's session; a switch of X's backends takes its claim, and the claim lapses inside the close of
/// X's kept session (the test's hook), the switch's first device step. The switch is refused
/// (`backend_switch_refused`, failed) with the problem `instance_fenced`, caused by the refusal
/// (`lease_expired`), and X's registration keeps its backends; the Runtime's health stays clean;
/// X alone is fenced, with `lease_expired` as its cause; the refusal is recorded once, under the
/// switch's request, naming X, the claim's lease and the stage `runtime_lifecycle_session_close`;
/// the claim's lease is released once (the switch's end of its key), never expired and never
/// handed on; X's session is closed once after that; Y then runs a direct run to its end, and the
/// host closes. Before the fix the refusal stopped the Runtime.
#[test]
fn close475_a_backend_switch_whose_key_lapsed_before_its_close_is_refused_and_fences_only_its_instance()
 {
    let fixture = Fixture::switching();
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let (switch, answered) = fixture.switch_backends(X);

    let refused = answered.expect("the switch's receipt");
    assert_eq!(refused.state(), RuntimeReceiptState::Failed, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        HostCode::BackendSwitchRefused.as_str(),
        "{refused:?}"
    );
    let outcome = fixture
        .host
        .failure_outcome_for_test(switch.request_id())
        .expect("refusal outcomes")
        .expect("the switch was refused");
    assert_eq!(
        outcome
            .value("backend_switch_problem")
            .and_then(Value::as_str),
        Some("instance_fenced"),
        "{outcome:?}"
    );
    assert!(
        outcome
            .causes()
            .iter()
            .any(|link| link.outcome().code().as_str() == RuntimeErrorCode::LeaseExpired.as_str()),
        "{outcome:?}"
    );
    assert!(
        fixture.states[X]
            .switches
            .lock()
            .expect("fake switches")
            .is_empty(),
        "X's registration keeps its backends"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    assert_eq!(refusal.links().request_id(), Some(&switch.request_id()));
    let lease = failure_outcome(&refusal)
        .value("lease_id")
        .cloned()
        .expect("the refusal names the switch's lease");
    fixture.assert_released_then_closed_once(X, mark, &lease);
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}

/// #475, the fourth place, a failure of the Runtime itself stays fatal: as above, with the record
/// of the refused close made to fail (#458's test-only switch). The Runtime holds `ledger_failure`
/// at `fence_instance_resources` latched, fatal and Global; no instance is fenced.
#[test]
fn close475_a_record_that_cannot_be_appended_at_a_backend_switchs_close_still_stops_the_runtime() {
    let fixture = Fixture::switching();
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let _answered = fixture.switch_backends(X);

    fixture.assert_record_failure_latched();
    drop(scan);
    fixture.close_unchecked();
}

/// #475 fix round 1, the fourth place at the switch's restore (`restore_backends`): X and Y in
/// #402 HS-2's shape; X's observation keeps X's session; the ledger refuses the switch's
/// `host.config_overrides` (`fail_next_config_persist_for_test`, as `gate_hs2.rs` refuses it), so
/// the candidate, which opened and passed, is not committed and the switch brings X's previous
/// backends back, whose first step closes the candidate's session on the switch's key. The claim
/// lapses inside that close (the test's hook, armed by the switch's first close, of X's kept
/// session). The switch is refused with the commit's failure (`host_config_overrides_persist_failed`,
/// failed), the restore's refusal (`lease_expired`) among its outcome's links; the registration
/// took the candidate first; the Runtime's health stays clean; X alone is fenced, with
/// `lease_expired` as its cause; the refusal is recorded once, under the switch's request, naming
/// X, the claim's lease and the stage `runtime_lifecycle_session_close`, after the switch's first
/// close; the claim's lease is released once (the switch's end of its key), never expired and
/// never handed on; X's session is closed once after that; Y then runs a direct run to its end,
/// and the host closes. Before the fix the refusal stopped the Runtime.
#[test]
fn close475_a_backend_switchs_restore_whose_key_lapsed_before_its_close_fences_only_its_instance() {
    let fixture = Fixture::switching();
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    fixture
        .host
        .fail_next_config_persist_for_test()
        .expect("the ledger refuses the next host.config_overrides");
    fixture.lapse_inside_the_close_after_the_next();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let (switch, answered) = fixture.switch_backends(X);

    let refused = answered.expect("the switch's receipt");
    assert_eq!(refused.state(), RuntimeReceiptState::Failed, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        crate::codes::ConfigHostCode::OverridesPersistFailed.as_str(),
        "{refused:?}"
    );
    let outcome = fixture
        .host
        .failure_outcome_for_test(switch.request_id())
        .expect("refusal outcomes")
        .expect("the switch was refused");
    assert!(
        outcome
            .causes()
            .iter()
            .any(|link| link.outcome().code().as_str() == RuntimeErrorCode::LeaseExpired.as_str()),
        "{outcome:?}"
    );
    let first_switch = fixture.states[X]
        .switches
        .lock()
        .expect("fake switches")
        .first()
        .map(|(touch, capture, _)| (touch.clone(), capture.clone()));
    assert_eq!(
        first_switch,
        Some((SWITCH_TO.0.to_owned(), SWITCH_TO.1.to_owned())),
        "the registration took the candidate: the switch reached its restore"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    assert_eq!(refusal.links().request_id(), Some(&switch.request_id()));
    let lease = failure_outcome(&refusal)
        .value("lease_id")
        .cloned()
        .expect("the refusal names the switch's lease");
    let events = fixture.events_after(mark);
    assert!(
        lease_events(&events, EventType::LeaseExpired, &lease).is_empty(),
        "the switch's end of its key ended the lease, never the sweep"
    );
    assert!(
        handed_on_from(&events, &lease).is_empty(),
        "the switch's key is handed to no one"
    );
    let releases = lease_events(&events, EventType::LeaseReleased, &lease);
    let [release] = releases.as_slice() else {
        panic!("the switch's lease is released once: {releases:#?}");
    };
    let closes = confirmed_closes(&events, fixture.ids[X]);
    let [kept, owed] = closes.as_slice() else {
        panic!(
            "X's kept session closed at the switch's first step, its candidate's after: {closes:?}"
        );
    };
    assert!(
        *kept < refusal.sequence()
            && refusal.sequence() < release.sequence()
            && release.sequence() < *owed,
        "the restore's close was refused after the first close, and X's session is closed once \
         after the lease ended: {closes:?}"
    );
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}

/// #475, the fifth place: the member switch, in a device group of several (which no installation
/// can configure today: the host is handed the group as the existing switch tests hand it). B's
/// client lease holds the device; A's application is stopped under B's key, and B's lease lapses
/// inside the close of the session the stop used (the test's hook). The stop fails
/// `application_switch_failed` at its step `stop`, with `lease_expired` as its step's code; the
/// Runtime's health stays clean; A alone is fenced (B and Y are not), with `lease_expired` as its
/// cause; the refusal is recorded once, naming A, B's lease and the stage
/// `runtime_lifecycle_session_close`. Then the key's end, one sweep tick, expires B's lapsed
/// lease once, hands the device to no one and closes A's session once after it; Y then runs a
/// direct run to its end. Before the fix the refusal stopped the Runtime.
#[test]
fn close475_a_member_switch_whose_key_lapsed_before_its_close_fences_only_the_stopped_member() {
    let fixture = Fixture::group_beside_y();
    let connection = fixture.connection();
    let b_lease = fixture.acquire_on(B, connection);
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();

    let failed = fixture
        .host
        .stop_member_application_for_test(&b_lease, connection, A)
        .expect("no failure of the Runtime's own")
        .expect_err("the refused close fails the stop");

    assert_eq!(
        failed.code(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "{failed:?}"
    );
    let outcome = failed.outcome();
    assert_eq!(
        outcome.value("application_switch_step"),
        Some(&json!("stop")),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(RuntimeErrorCode::LeaseExpired.as_str())),
        "{outcome:?}"
    );
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(A, mark);
    assert_eq!(
        failure_outcome(&refusal).value("lease_id"),
        Some(&lease_value(&b_lease.lease_id()))
    );
    drop(scan);
    fixture.tick();
    fixture.assert_runtime_runs();
    let events = fixture.events_after(mark);
    let expiries = lease_records(&events, EventType::LeaseExpired, b_lease.lease_id());
    let [expiry] = expiries.as_slice() else {
        panic!("B's lease expired once: {expiries:#?}");
    };
    let closes = confirmed_closes(&events, fixture.ids[A]);
    assert!(
        closes.len() == 1 && closes[0] > expiry.sequence(),
        "A's session is closed once, after B's lease ended: {closes:?}"
    );
    assert!(
        !fixture.transferred_since(mark, A) && !fixture.transferred_since(mark, B),
        "the device is handed to no one"
    );
    let log = fixture.sessions();
    assert_eq!(open_backends(&log, A), 0, "A's session is closed: {log:?}");
    fixture.assert_y_runs();
    // A's fence stays: the Host close of it is not this test's subject.
    fixture.close_unchecked();
}

/// #475, the fifth place, a failure of the Runtime itself stays fatal: as above, with the record
/// of the refused close made to fail (#458's test-only switch). The stop returns that failure,
/// `ledger_failure` at `fence_instance_resources`, fatal and Global; no instance is fenced.
#[test]
fn close475_a_record_that_cannot_be_appended_at_a_member_switchs_close_still_stops_the_runtime() {
    let fixture = Fixture::group_beside_y();
    let connection = fixture.connection();
    let b_lease = fixture.acquire_on(B, connection);
    fixture
        .host
        .fail_refused_close_record_for_test(true)
        .expect("the record's failure");
    fixture.lapse_inside_the_next_close();
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let failure = fixture
        .host
        .stop_member_application_for_test(&b_lease, connection, A)
        .expect_err("the stop returns the record's failure");

    assert_record_failure(&failure);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    drop(scan);
    fixture.close_unchecked();
}

/// #475 (the review of #461's fix, item 3): while a close is owed the device is handed to no one,
/// a claim waiting in the queue included. X's observation opens X's session, a client's lease of
/// X holds X and a person's self-check of X waits behind it (a claim X's fence does not hold back);
/// the lease lapses with the sweep's expiry scan held, and the client's connection closes. The
/// connection's end returns no error and the Runtime's health stays clean; X alone is fenced, with
/// `lease_expired` as its cause; the refusal is recorded once, naming X's lease; X's lease is
/// released once, never expired and never handed on (no transfer to the waiting claim); X's
/// session is closed once after that, and only then is the waiting claim granted, by the queue's
/// own grant (admitted, granted).
#[test]
fn close475_a_disconnects_owed_close_is_made_before_the_waiting_claim_is_granted() {
    let fixture = Fixture::alone(&alone_ids(), false);
    let observed = fixture.observe(X).1;
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let connection = fixture.connection();
    let x_lease = fixture.acquire_on(X, connection);
    let x_waiting = fixture.claim(X, ClaimKind::SelfCheck);
    assert!(
        x_waiting.queued.is_some() && x_waiting.granted.is_none(),
        "X's self-check waits behind X's lease: {x_waiting:?}"
    );
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");
    let mark = fixture.head();
    fixture.lapse_past(&x_lease);

    let ended = fixture.host.end_connection_for_test(connection);

    assert!(ended.is_ok(), "the connection's end: {ended:?}");
    fixture.assert_runtime_runs();
    let refusal = fixture.assert_fenced_by_one_refusal(X, mark);
    let lease = lease_value(&x_lease.lease_id());
    assert_eq!(failure_outcome(&refusal).value("lease_id"), Some(&lease));
    let close = fixture.assert_released_then_closed_once(X, mark, &lease);
    let token = x_waiting
        .wait_for_grant(GATE_WAIT)
        .expect("the grant slot")
        .expect("X's waiting claim is granted after X's close");
    assert_eq!(token.instance_id(), fixture.ids[X]);
    let events = fixture.events_after(mark);
    assert!(
        close < grant_of(&events, &x_waiting),
        "X's session is closed before the waiting claim's lease.granted"
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
        "no transfer handed the waiting claim the device: {types:?}"
    );
    fixture
        .host
        .release_host_claim_for_test(&x_waiting, &token)
        .expect("the self-check's lease end");
    drop(scan);
    fixture.assert_y_runs();
    fixture.close();
}
