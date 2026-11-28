// SPDX-License-Identifier: AGPL-3.0-only

//! #503 ISO-A (0.12.102; register §1.554; board #419, the owner's rule of 10-24: one instance's
//! fault does not stop the whole Runtime): the instance worker contains its payloads' failures and
//! panics, and the Runtime's own availability record of an instance is never refused as not newer.
//!
//! - h1: two instances X and Y. A payload of X's worker ends with an instance's error, then with
//!   an operation's error, then panics while it holds X's admission guard and queue-order lock,
//!   then panics a second time before the rebuilt worker finished a payload. After each, a payload
//!   posted to each worker runs, X is fenced or failed as the step says (the fence's records, one
//!   `runtime.failed` record of the operation), and the Runtime latches nothing. After the panic
//!   holding X's locks they are no longer poisoned, and a person's self-check of X, which takes X's
//!   admission guard, passes and lifts the fence.
//! - h2: a probe of an unavailable instance whose payload ends with an error its worker contains
//!   tells a person at once with the error's code, probes no more, and the worker goes on.
//! - h3: a payload's failure of the whole (a ledger failure) latches the Runtime as before.
//! - h4: a panic while holding a lock every instance shares (the scheduler's) still latches
//!   `runtime_state_poisoned` at the next touch, and fences nothing.
//! - h5: the policy instance's availability is withdrawn and restored twice in one millisecond,
//!   at its start and at a person's self-check; each write is taken, one millisecond after the one
//!   before, and nothing latches.
//! - h6: after the wall clock stepped back a second, a person's self-check withdraws and restores
//!   the availability, a fence withdraws it and the fence completes, and the next passing self-check
//!   lifts the fence; each write is taken after the stored one.
//!
//! The hosts run on a manual Runtime clock; the waits only bound a broken run.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::{all_events, wait_until};
use super::*;
use crate::codes::HostCode;
use crate::codes::test_spellings::TestLocation;
use crate::host::failure_scope::{Contained, WorkerActForTest};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactPayload, InstanceStuckRecovery, RuntimeConfigManifest, RuntimePayload,
    SchedulingResumeSelfCheck,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const X_ALIAS: &str = "isoa.instance.x";
const Y_ALIAS: &str = "isoa.instance.y";
/// The connection a person's self-check comes on.
const SELF_CHECK_CONNECTION: u64 = 5_031;

/// A started host with X and Y on a manual clock. X's stuck recovery is off, so its fence waits
/// for a person and no ladder acts on X; X's opens pass a person's self-check.
struct Pair {
    host: RuntimeHost,
    x: InstanceId,
    _root: TempDir,
}

fn pair() -> Pair {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    x_state.observed_open.store(true, Ordering::Release);
    let x = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(FakeProvider::from_entries([
            (X_ALIAS.to_owned(), x, x_state),
            (
                Y_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("runtime host");
    Pair {
        host,
        x,
        _root: root,
    }
}

impl Pair {
    /// The Runtime holds no fatal state and no shutdown was requested.
    fn assert_running(&self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime latched"
        );
        assert!(
            !self.host.is_shutdown_requested().expect("shutdown state"),
            "a shutdown was requested"
        );
    }

    /// The Runtime latched `code`, and a shutdown was requested.
    fn assert_latched(&self, code: &str) {
        let latched = self
            .host
            .fatal_error()
            .expect("runtime health")
            .expect("the Runtime latched");
        assert_eq!(latched.code(), code, "{latched:?}");
        assert!(self.host.is_shutdown_requested().expect("shutdown state"));
    }

    /// A payload posted to X's worker and one posted to Y's each run.
    fn assert_both_workers_run(&self) {
        assert_worker_runs(&self.host, X_ALIAS);
        assert_worker_runs(&self.host, Y_ALIAS);
    }

    /// Waits for the `count`-th fence record: it fences X, caused by `cause`, and the status shows
    /// X fenced so and Y not.
    fn assert_x_fenced(&self, count: usize, cause: &str) {
        wait_until("X's fence", || {
            fence_records(&all_events(&self.host)).len() == count
        });
        let events = all_events(&self.host);
        let fences = fence_records(&events);
        let (record, outcome) = fences.last().expect("the fence record");
        assert_eq!(record.links().instance_id(), Some(&self.x));
        assert_fence(outcome, self.x, cause);
        assert_eq!(
            status_fence(&self.host, X_ALIAS).map(|fence| fence.cause.as_str().to_owned()),
            Some(cause.to_owned())
        );
        assert_eq!(status_fence(&self.host, Y_ALIAS), None);
    }

    /// A person's self-check of X, which takes X's admission guard, passes and lifts its fence.
    fn assert_x_lifted(&self) {
        let check = self_check(&self.host, X_ALIAS);
        assert!(passed(&check), "{check:?}");
        assert_eq!(
            status_fence(&self.host, X_ALIAS),
            None,
            "the fence is lifted"
        );
    }
}

/// A payload posted to the worker of `alias`'s device is taken and ends.
fn assert_worker_runs(host: &RuntimeHost, alias: &str) {
    let done = Arc::new(AtomicBool::new(false));
    host.post_worker_payload_for_test(alias, WorkerActForTest::Finish(Arc::clone(&done)))
        .expect("the worker takes the payload");
    wait_until("the worker's payload", || done.load(Ordering::Acquire));
}

/// A person's self-check of `alias` (`actingctl selfcheck`), completed: its self-check.
fn self_check(host: &RuntimeHost, alias: &str) -> SchedulingResumeSelfCheck {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = host
        .process_request_for_test(
            &runtime_request(
                &ids,
                RuntimeOperation::SelfCheckInstance {
                    instance_alias: alias.to_owned(),
                },
            ),
            ConnectionId::new(SELF_CHECK_CONNECTION).expect("connection"),
        )
        .expect("self-check receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    selfcheck.clone()
}

fn passed(check: &SchedulingResumeSelfCheck) -> bool {
    check.capture.ok && check.touch.ok && check.failure_code.is_none()
}

/// The `runtime.failed` records whose outcome's code is `spelling`, with their outcomes, in
/// ledger order.
fn failed_with<'a>(
    events: &'a [PersistedEvent],
    spelling: &str,
) -> Vec<(&'a PersistedEvent, &'a Outcome)> {
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == spelling =>
            {
                Some((event, record.outcome()))
            }
            _ => None,
        })
        .collect()
}

fn value(outcome: &Outcome, key: &str) -> Value {
    outcome.value(key).cloned().unwrap_or(Value::Null)
}

/// The failure of one operation, as the fact store refuses a record that is not newer.
fn operation_failure() -> RuntimeHostError {
    RuntimeHostError::request(
        HostCode::FactObservationNotNewer,
        ContractLocation::PublishFact,
        RuntimeErrorCode::InvalidRequest,
    )
}

/// h1: one instance's worker fails and panics; only that instance is held, and every payload of
/// both workers after each step runs.
#[test]
fn iso_a_h1_one_instances_worker_failures_and_panics_hold_only_that_instance() {
    let fixture = pair();

    // An instance's error (its capture backend failed, its state unknown): X is fenced, with the
    // fence's own record caused by it.
    fixture
        .host
        .post_worker_payload_for_test(
            X_ALIAS,
            WorkerActForTest::Fail(RuntimeHostError::instance_fault(
                SharedCode::CaptureBackendOperationFailed,
                TestLocation::ContainFailureForTest,
                RuntimeErrorCode::CaptureFailed,
                fixture.x,
            )),
        )
        .expect("post X's failing payload");
    fixture.assert_x_fenced(1, SharedCode::CaptureBackendOperationFailed.as_str());
    fixture.assert_running();
    fixture.assert_both_workers_run();
    fixture.assert_x_lifted();

    // An operation's error: one record of it under X, nothing fenced.
    fixture
        .host
        .post_worker_payload_for_test(X_ALIAS, WorkerActForTest::Fail(operation_failure()))
        .expect("post X's failing payload");
    let not_newer = HostCode::FactObservationNotNewer.as_str();
    wait_until("the operation's failure record", || {
        !failed_with(&all_events(&fixture.host), not_newer).is_empty()
    });
    let events = all_events(&fixture.host);
    let failures = failed_with(&events, not_newer);
    let [(failure, _)] = failures.as_slice() else {
        panic!("one record of the operation's failure: {failures:#?}");
    };
    assert_eq!(failure.links().instance_id(), Some(&fixture.x));
    assert_eq!(fence_records(&events).len(), 1, "nothing more is fenced");
    assert!(
        failed_with(&events, HostCode::InstanceWorkerUnavailable.as_str()).is_empty(),
        "X's worker is there"
    );
    assert_eq!(status_fence(&fixture.host, X_ALIAS), None);
    fixture.assert_running();
    fixture.assert_both_workers_run();

    // A panic while X's admission guard and queue-order lock are held: its record, then X's fence
    // caused by `instance_lock_poisoned`; neither lock is poisoned any more.
    fixture
        .host
        .post_worker_payload_for_test(X_ALIAS, WorkerActForTest::PanicHoldingInstanceLocks)
        .expect("post X's panicking payload");
    fixture.assert_x_fenced(2, HostCode::InstanceLockPoisoned.as_str());
    let events = all_events(&fixture.host);
    let panics = failed_with(&events, "panic_caught");
    let [(panic_record, _)] = panics.as_slice() else {
        panic!("one panic record: {panics:#?}");
    };
    let fences = fence_records(&events);
    let (fence_record, _) = fences.last().expect("the fence record");
    assert_eq!(panic_record.links().instance_id(), Some(&fixture.x));
    assert!(
        panic_record.sequence() < fence_record.sequence(),
        "the panic's record, then its fence"
    );
    assert!(
        !fixture
            .host
            .instance_admission_for_test(X_ALIAS)
            .expect("X's admission guard")
            .is_poisoned(),
        "X's admission guard is cleared"
    );
    assert!(
        !fixture
            .host
            .queue_order_lock_for_test(X_ALIAS)
            .expect("X's queue-order lock")
            .is_poisoned(),
        "X's queue-order lock is cleared"
    );
    fixture.assert_running();
    assert_worker_runs(&fixture.host, Y_ALIAS);
    fixture.assert_x_lifted();
    fixture.assert_running();

    // The rebuilt worker panics again before it finished a payload: X is fenced (`panic_caught`)
    // instead of the whole Runtime asked to restart, and both workers go on.
    fixture
        .host
        .post_worker_payload_for_test(X_ALIAS, WorkerActForTest::Panic)
        .expect("post X's second panicking payload");
    fixture.assert_x_fenced(3, "panic_caught");
    fixture.assert_running();
    fixture.assert_both_workers_run();
    assert_eq!(
        failed_with(&all_events(&fixture.host), "panic_caught").len(),
        2,
        "each panic is recorded once"
    );
    fixture.host.close().expect("close host");
}

/// The defaults file's configuration with `file`'s sections, as actingd assembles it.
fn manifest(file: &Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, file, &|_| None)
        .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn now_unix_ms(clock: &ManualRuntimeClock) -> u64 {
    clock.sample().expect("manual clock").unix_ms
}

/// h2: a probe's payload ends with an operation's error: the probe ends as a panicked one does,
/// a person is told at once with the error's code, nothing probes the instance again, and the
/// worker goes on.
#[test]
fn iso_a_h2_a_probe_whose_payload_fails_tells_a_person_with_the_failures_code() {
    const CAP_MS: u64 = 1_800_000;
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let registered = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(manifest(&json!({
                "scheduling": {
                    "unavailable_probe": {
                        "first_interval_ms": 60_000,
                        "factor": 2,
                        "max_interval_ms": CAP_MS,
                        "attention_after_ms": 3_600_000,
                    }
                }
            })))
            .with_stuck_recovery(BTreeMap::from([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, Arc::clone(&state))
                .with_device_endpoints(),
        ),
    )
    .expect("runtime host");
    // A person's self-check whose capture open fails makes the instance one the probes take up.
    state.observed_open.store(true, Ordering::Release);
    *state.capture_open_error.lock().expect("capture open error") = Some(
        DeviceError::fatal("fixture capture open got no positive connection handle")
            .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1),
    );
    assert_eq!(
        self_check(&host, POLICY_INSTANCE_ALIAS)
            .failure_code
            .as_deref(),
        Some("capture_backend_open_failed")
    );
    let unavailable = host
        .unavailable_for_test(POLICY_INSTANCE_ALIAS)
        .expect("the probes' state")
        .expect("the instance is unavailable");
    let next = unavailable.next_probe_unix_ms.expect("a next probe");
    host.fail_next_host_claim_for_test(POLICY_INSTANCE_ALIAS, operation_failure())
        .expect("arm the probe's failure");
    clock.advance(next.saturating_sub(now_unix_ms(&clock)));
    let probed_at = now_unix_ms(&clock);
    host.unavailable_probe_tick_for_test()
        .expect("the probes' tick");

    let told_code = HostCode::InstanceUnavailableAwaitingPerson.as_str();
    wait_until("a person is told", || {
        !failed_with(&all_events(&host), told_code).is_empty()
    });
    let events = all_events(&host);
    let failures = failed_with(&events, HostCode::FactObservationNotNewer.as_str());
    let [(failure, _)] = failures.as_slice() else {
        panic!("one record of the probe's failure: {failures:#?}");
    };
    assert_eq!(failure.links().instance_id(), Some(&registered));
    let told = failed_with(&events, told_code);
    let [(told_record, outcome)] = told.as_slice() else {
        panic!("one attention: {told:#?}");
    };
    assert_eq!(told_record.links().instance_id(), Some(&registered));
    assert_eq!(
        (
            value(outcome, "failure_code"),
            value(outcome, "attempts"),
            value(outcome, "elapsed_ms"),
            value(outcome, "next_occurrence_unix_ms"),
        ),
        (
            json!(HostCode::FactObservationNotNewer.as_str()),
            json!(1),
            json!(probed_at - unavailable.since_unix_ms),
            Value::Null,
        )
    );
    assert!(failed_with(&events, "panic_caught").is_empty(), "no panic");
    let held = |host: &RuntimeHost| {
        host.unavailable_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the probes' state")
            .map(|state| {
                (
                    state.probes,
                    state.next_probe_unix_ms,
                    state.awaiting_person,
                )
            })
    };
    assert_eq!(held(&host), Some((1, None, true)), "no next probe");
    assert_worker_runs(&host, POLICY_INSTANCE_ALIAS);
    clock.advance(CAP_MS * 10);
    host.unavailable_probe_tick_for_test()
        .expect("the probes' tick");
    assert_eq!(held(&host), Some((1, None, true)), "nothing was queued");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

/// h3: a payload's failure of the whole Runtime (a ledger failure) latches it, as before, and
/// fences nothing.
#[test]
fn iso_a_h3_a_payload_failure_of_the_whole_latches_the_runtime() {
    let fixture = pair();
    let ledger = RuntimeErrorCode::LedgerFailure.as_str();
    fixture
        .host
        .post_worker_payload_for_test(
            X_ALIAS,
            WorkerActForTest::Fail(RuntimeHostError::fatal(
                RuntimeErrorCode::LedgerFailure,
                TestLocation::ContainFailureForTest,
                RuntimeErrorCode::LedgerFailure,
            )),
        )
        .expect("post X's failing payload");
    wait_until("the latch", || {
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_some()
    });
    fixture.assert_latched(ledger);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    let closed = fixture
        .host
        .close()
        .expect_err("the close ends with the latched condition");
    assert_eq!(closed.code(), ledger);
}

/// h4: a panic while the scheduler's lock, which every instance shares, is held leaves it
/// poisoned: the next touch of it latches `runtime_state_poisoned`, and nothing is fenced.
#[test]
fn iso_a_h4_a_panic_holding_a_shared_lock_latches_at_the_next_touch() {
    let fixture = pair();
    fixture
        .host
        .post_worker_payload_for_test(X_ALIAS, WorkerActForTest::PanicHoldingSharedLock)
        .expect("post X's panicking payload");
    wait_until("the shared lock's poison", || {
        fixture.host.instance_claims_for_test(Y_ALIAS).is_err()
    });
    // A person's self-check of Y is one next touch (its claim takes the scheduler's lock); the
    // lease sweep is another. Either latches.
    drop(fixture.host.process_request_for_test(
        &runtime_request(
            &IdentifierIssuer::new().expect("identifier issuer"),
            RuntimeOperation::SelfCheckInstance {
                instance_alias: Y_ALIAS.to_owned(),
            },
        ),
        ConnectionId::new(SELF_CHECK_CONNECTION).expect("connection"),
    ));
    wait_until("the latch", || {
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_some()
    });
    fixture.assert_latched(HostCode::RuntimeStatePoisoned.as_str());
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    assert!(fixture.host.close().is_err(), "the Runtime latched");
}

/// The policy instance on a bound device endpoint (its availability gated on its self-check),
/// every open passing, on a manual clock that only the test moves (no open moves it), its stuck
/// recovery off; started, its start's restore of the availability recorded.
struct Gated {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    registered: InstanceId,
    _root: TempDir,
}

fn gated() -> Gated {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let registered = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_stuck_recovery(BTreeMap::from([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state).with_device_endpoints(),
        ),
    )
    .expect("runtime host");
    wait_until("the start's restore", || {
        availability(&host)
            .last()
            .is_some_and(|(available, _)| *available)
    });
    Gated {
        host,
        clock,
        registered,
        _root: root,
    }
}

/// Every publication of the policy instance's `session.instance.available`: its value and its
/// observation, in ledger order.
fn availability(host: &RuntimeHost) -> Vec<(bool, u64)> {
    let scope = FactScope::Instance {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
    };
    all_events(host)
        .iter()
        .filter_map(|event| {
            let EventPayload::Fact(FactPayload::Published(payload)) = event.payload() else {
                return None;
            };
            let record = payload.record();
            if record.key != crate::fact_store::POLICY_INSTANCE_AVAILABLE_KEY
                || record.scope != scope
            {
                return None;
            }
            match record.content {
                FactContent::Inline {
                    value: ContractFactValue::Boolean(available),
                } => Some((available, record.observed_at_unix_ms)),
                _ => None,
            }
        })
        .collect()
}

/// Each publication is observed after the one before.
fn assert_increasing(publications: &[(bool, u64)]) {
    assert!(
        publications
            .windows(2)
            .all(|adjacent| adjacent[0].1 < adjacent[1].1),
        "{publications:?}"
    );
}

/// The last two publications are a withdrawal and its restore, one millisecond after it.
fn assert_restored_after_withdrawal(publications: &[(bool, u64)]) {
    let [.., (false, withdrawn), (true, restored)] = publications else {
        panic!("a withdrawal, then its restore: {publications:?}");
    };
    assert_eq!(*restored, withdrawn + 1, "{publications:?}");
}

/// h5: two availability writes in one millisecond: the start's withdrawal and restore, then a
/// person's self-check's at the same millisecond, each taken one millisecond after the one before;
/// nothing latches.
#[test]
fn iso_a_h5_two_availability_writes_in_one_millisecond_are_both_taken() {
    let fixture = gated();
    let started = availability(&fixture.host);
    assert_increasing(&started);
    assert_restored_after_withdrawal(&started);
    assert_eq!(
        now_unix_ms(&fixture.clock),
        POLICY_NOW_UNIX_MS,
        "no open moved the clock"
    );

    let check = self_check(&fixture.host, POLICY_INSTANCE_ALIAS);
    assert!(passed(&check), "{check:?}");
    let checked = availability(&fixture.host);
    assert!(checked.len() >= started.len() + 2, "{checked:?}");
    assert_increasing(&checked);
    assert_restored_after_withdrawal(&checked);
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "the Runtime latched"
    );
    fixture.host.close().expect("close host");
}

/// h6: after the wall clock stepped back a second, a person's self-check's withdrawal and restore
/// are taken after the stored record; a fence then withdraws the availability and completes (ISO-M
/// row F29), and the next passing self-check lifts it and restores the availability.
#[test]
fn iso_a_h6_writes_after_a_clock_step_back_are_taken_and_a_fence_completes() {
    let fixture = gated();
    let stored = availability(&fixture.host)
        .last()
        .map(|(_, observed)| *observed)
        .expect("the start's restore");
    fixture.clock.set_unix_ms(POLICY_NOW_UNIX_MS - 1_000);

    let check = self_check(&fixture.host, POLICY_INSTANCE_ALIAS);
    assert!(passed(&check), "{check:?}");
    let checked = availability(&fixture.host);
    assert_increasing(&checked);
    assert_restored_after_withdrawal(&checked);
    let restored = checked
        .last()
        .map(|(_, observed)| *observed)
        .expect("the self-check's restore");
    assert!(restored > stored, "{checked:?}");

    let contained = fixture
        .host
        .contain_failure_for_test(
            RuntimeHostError::instance_fault(
                SharedCode::CaptureBackendOperationFailed,
                TestLocation::ContainFailureForTest,
                RuntimeErrorCode::CaptureFailed,
                fixture.registered,
            ),
            None,
        )
        .expect("contain the instance's failure");
    assert!(matches!(contained, Contained::Fenced(_)), "{contained:?}");
    let fenced = availability(&fixture.host);
    assert_eq!(
        fenced.last(),
        Some(&(false, restored + 1)),
        "the fence's withdrawal follows the stored record: {fenced:?}"
    );

    let lift = self_check(&fixture.host, POLICY_INSTANCE_ALIAS);
    assert!(passed(&lift), "{lift:?}");
    assert_eq!(status_fence(&fixture.host, POLICY_INSTANCE_ALIAS), None);
    let lifted = availability(&fixture.host);
    assert_increasing(&lifted);
    assert!(
        lifted.last().is_some_and(|(available, _)| *available),
        "{lifted:?}"
    );
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none(),
        "the Runtime latched"
    );
    fixture.host.close().expect("close host");
}
