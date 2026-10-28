// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-5a (Workflow #419; ISO-M rows A08, A11, B02; coordinator ruling C-43 point 3): no step of a
//! run's end, or of a lease holder's end, may stop the Runtime.
//!
//! - A run that ends otherwise than at its deadline (here a success) enters its end phase too:
//!   while it holds its lease, its lease is renewed once by the end grace
//!   (`host_claims.run_end_grace_ms`, default 120 000 ms) when it would lapse inside the grace, and
//!   its terminal and its release run under that lease. A success whose end work passes the
//!   lease's original end settles once as a success; nothing stops (rows A08, A11).
//! - A success whose lease the sweep ended after the end grace still settles once, as a success
//!   (its sole terminal is `task.completed`), never as `policy_run_failure_terminal_conflict`;
//!   only the run's receipt fails, non-fatal (row A11).
//! - A lease expiry that meets a write still in flight on the lease (an input writes without the
//!   admission guard) closes nothing and records nothing; the sweep retries it, and the first
//!   tick after the write ends the lease once (row B02).
//!
//! Two instances, X (the policy instance) and Y, on fakes and a Runtime clock that moves only
//! when the kernel waits or the test moves it; every move samples business capacity, as the
//! capacity monitor does while time passes (the `gate_411_a64.rs` clock). No sleep, no wall
//! clock.

use super::gate_403::{fence_records, status_fence};
use super::instance_workers::all_events;
use super::*;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::InstanceStuckRecovery;
use std::sync::{Mutex, OnceLock};

const X_ALIAS: &str = POLICY_INSTANCE_ALIAS;
const Y_ALIAS: &str = "iso5a.instance.other";
/// X's scheduled run's response deadline: its lease lasts this plus the heartbeat reserve, less
/// than the end grace.
const RESPONSE_DEADLINE_MS: u64 = 50_000;
/// Ruling C-43: the end grace in effect, the default of `host_claims.run_end_grace_ms`.
const END_GRACE_MS: u64 = 120_000;

/// Samples business capacity once and records it (`RuntimeHost::capacity_sampler_for_test`).
type CapacitySampler = Box<dyn Fn() -> RuntimeHostResult<()> + Send>;

/// The Runtime clock of these tests: it moves only when the contained-task kernel waits, by the
/// wait rounded up to the millisecond, or when the test moves it; every move samples capacity.
struct SampledClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
    sampler: OnceLock<Mutex<CapacitySampler>>,
}

impl SampledClock {
    fn new(unix_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
            sampler: OnceLock::new(),
        }
    }

    /// Every later move samples capacity with `sampler`; set once, right after the host starts.
    fn sample_capacity_after_moves(&self, sampler: CapacitySampler) {
        assert!(
            self.sampler.set(Mutex::new(sampler)).is_ok(),
            "the capacity sampler is set once"
        );
    }

    fn advance(&self, duration_ms: u64) {
        self.unix_ms.fetch_add(duration_ms, Ordering::SeqCst);
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
        if let Some(sampler) = self.sampler.get() {
            let sample_capacity = sampler.lock().expect("the capacity sampler's lock");
            (*sample_capacity)().expect("capacity sample at the time the clock moved to");
        }
    }

    fn monotonic_ms(&self) -> u64 {
        self.monotonic_ms.load(Ordering::SeqCst)
    }
}

impl RuntimeClock for SampledClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.load(Ordering::SeqCst),
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }

    fn pause(&self, duration: Duration) {
        self.advance(u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX));
    }
}

/// The started host with X and Y; with `admit`, X's scheduled run is admitted.
struct Fixture {
    _root: TempDir,
    host: Arc<RuntimeHost>,
    clock: Arc<SampledClock>,
    x: InstanceId,
    x_state: Arc<FakeState>,
    y_package: String,
    /// X's admitted scheduled run and its request (a run that succeeds).
    run: Option<(Box<PolicyRunContext>, ContainedTaskRequest)>,
}

fn fixture(admit: bool) -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    let y_state = Arc::new(FakeState::default());
    // A one-tap package whose target page comes after the tap: each instance's run succeeds.
    let x_package = super::gate_369s5::direct_run_package(&root, &x_state);
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let x = instance_id();
    let y = instance_id();
    // The policy's configured identities are the registered aliases; only X is available to it.
    let mut facts = policy_facts();
    let mut y_identity = facts.instances[0].clone();
    y_identity.instance_id = Y_ALIAS.to_owned();
    y_identity.available = false;
    facts.instances.push(y_identity);
    let provider = FakeProvider::from_entries([
        (X_ALIAS.to_owned(), x, Arc::clone(&x_state)),
        (Y_ALIAS.to_owned(), y, Arc::clone(&y_state)),
    ]);
    let clock = Arc::new(SampledClock::new(POLICY_NOW_UNIX_MS));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig::default())
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_procedure_manifest(procedure_manifest_with_primary(
                &neutral_contained_task_package(true),
                vec!["after_observation".to_owned()],
            ))
            .with_scheduled_procedures(["procedure.observe".to_owned()])
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(provider),
    )
    .expect("runtime host");
    clock.sample_capacity_after_moves(host.capacity_sampler_for_test().expect("capacity owner"));
    clock.advance(0);
    let run = admit.then(|| {
        host.activate_policy_catalog(&policy_sources(1))
            .expect("activate policy catalog");
        let cycle = host
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
            .expect("evaluate policy dispatch");
        let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.instance_id == X_ALIAS)
            .expect("X's dispatch intent")
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("dispatch reason chain")
            .clone();
        record_policy_approval(&host, &intent);
        let request = ContainedTaskRequest::new(x_package.clone())
            .expect("contained task request")
            .with_response_deadline_ms(RESPONSE_DEADLINE_MS)
            .expect("bounded response deadline");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&host, &intent),
                &request,
            )
            .expect("policy admission")
        else {
            panic!("expected a policy run context")
        };
        (context, request)
    });
    Fixture {
        _root: root,
        host: Arc::new(host),
        clock,
        x,
        x_state,
        y_package,
        run,
    }
}

/// The records of `event_type` linked to `lease_id`.
fn lease_records(host: &RuntimeHost, event_type: EventType, lease_id: LeaseId) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        lease_id: Some(lease_id),
        ..EventQuery::default()
    })
    .expect("the lease's records")
    .len()
}

/// The `lease.expired` records of `lease_id`.
fn lease_expiries(host: &RuntimeHost, lease_id: LeaseId) -> usize {
    lease_records(host, EventType::LeaseExpired, lease_id)
}

/// The records of `event_type` linked to the run `context`.
fn run_records(host: &RuntimeHost, event_type: EventType, context: &PolicyRunContext) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        run_id: Some(context.run_id()),
        ..EventQuery::default()
    })
    .expect("the run's records")
    .len()
}

/// Nothing latched and no stop was requested.
fn assert_runtime_runs(host: &RuntimeHost) {
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime latched"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown state"),
        "a stop was requested"
    );
}

impl Fixture {
    /// Y is not fenced, and its direct run is dispatched and completes.
    fn assert_y_runs(&self) {
        assert_eq!(status_fence(&self.host, Y_ALIAS), None);
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let y_run = runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                Y_ALIAS,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.y_package.clone()).expect("task request"),
            ),
        );
        let receipt = self
            .host
            .process_request_for_test(&y_run, ConnectionId::new(6411).expect("connection"))
            .expect("Y's run receipt");
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
        assert_runtime_runs(&self.host);
    }

    /// Closes the host once every hook that shared it has run.
    fn close(self) {
        let Ok(host) = Arc::try_unwrap(self.host) else {
            panic!("a hook still holds the host");
        };
        host.close().expect("close host");
    }
}

/// a (rows A08, A11; C-43 point 3): X's scheduled run succeeds, and its end work runs past its
/// lease's original end: at `BeforeTerminalAppend` the clock moves past it, with the lease sweep
/// held. The run had entered its end phase at the start of its end work and its lease was renewed
/// once by the end grace, so its success terminal and its release run under that lease: the run
/// completes, settles once as a success (one `policy.execution_recorded`), its lease is released
/// once and never expired, nothing latches, X is not fenced and Y's run completes.
#[test]
fn iso5a_a_a_success_whose_end_passes_its_lease_end_settles_once_and_the_runtime_runs() {
    let mut fixture = fixture(true);
    let (context, request) = fixture.run.take().expect("X's admitted run");
    let lease = context.lease_token().clone();
    let lease_end = lease.expires_at_monotonic_ms();
    assert!(
        lease_end < fixture.clock.monotonic_ms() + END_GRACE_MS,
        "the admitted lease would lapse inside the end grace"
    );
    let hook_clock = Arc::clone(&fixture.clock);
    let past_lease_end = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::BeforeTerminalAppend,
            fixture.x,
            lease.lease_id(),
            move |_| {
                let now = hook_clock.monotonic_ms();
                hook_clock.advance(lease_end.saturating_sub(now) + 1);
            },
        )
        .expect("arm the end past the lease's end");
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let receipt = fixture
        .host
        .run_scheduled_contained_task(&context, &request)
        .expect("the run completes");
    drop(scan);

    assert_eq!(past_lease_end.consumed(), 1, "the run reached its terminal");
    assert!(
        fixture.clock.monotonic_ms() > lease_end,
        "the end passed the lease's original end"
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
    fixture
        .host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("the run settles as a success");
    assert_runtime_runs(&fixture.host);
    assert_eq!(
        run_records(&fixture.host, EventType::PolicyExecutionRecorded, &context),
        1,
        "one policy execution record"
    );
    assert_eq!(
        run_records(&fixture.host, EventType::TaskCompleted, &context),
        1
    );
    assert_eq!(
        lease_records(&fixture.host, EventType::LeaseRenewed, lease.lease_id()),
        1,
        "the end grace renewed the lease once"
    );
    assert_eq!(
        lease_records(&fixture.host, EventType::LeaseReleased, lease.lease_id()),
        1,
        "the run released its lease once"
    );
    assert_eq!(lease_expiries(&fixture.host, lease.lease_id()), 0);
    assert_eq!(status_fence(&fixture.host, X_ALIAS), None);
    fixture.assert_y_runs();
    fixture.close();
}

/// b (row A11, past the end grace): X's scheduled run succeeds, and at `BeforeTerminalAppend` its
/// end work has run past the end grace: a sweep tick there ends its lease. Its success terminal
/// still comes, and its release meets the ended lease. The run's sole terminal is
/// `task.completed`, so it settles once as a success (never `policy_run_failure_terminal_conflict`);
/// only its receipt fails, non-fatal; nothing latches and Y's run completes.
#[test]
fn iso5a_b_a_success_whose_lease_ended_after_the_grace_settles_as_a_success() {
    let mut fixture = fixture(true);
    let (context, request) = fixture.run.take().expect("X's admitted run");
    let lease_id = context.lease_token().lease_id();
    let expiries = Arc::new(AtomicUsize::new(usize::MAX));
    let (hook_host, hook_clock, hook_expiries) = (
        Arc::clone(&fixture.host),
        Arc::clone(&fixture.clock),
        Arc::clone(&expiries),
    );
    let past_grace = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::BeforeTerminalAppend,
            fixture.x,
            lease_id,
            move |_| {
                hook_clock.advance(END_GRACE_MS + 1);
                hook_host
                    .expire_due_leases_for_test()
                    .expect("a sweep tick after the end grace");
                hook_expiries.store(lease_expiries(&hook_host, lease_id), Ordering::SeqCst);
            },
        )
        .expect("arm the end past the end grace");

    let error = fixture
        .host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the release meets the ended lease");

    assert_eq!(past_grace.consumed(), 1, "the run reached its terminal");
    assert_eq!(
        expiries.load(Ordering::SeqCst),
        1,
        "the sweep ended the lease after the end grace"
    );
    assert!(!error.is_fatal(), "{error:?}");
    assert_runtime_runs(&fixture.host);
    assert_eq!(
        run_records(&fixture.host, EventType::TaskCompleted, &context),
        1
    );
    assert_eq!(
        run_records(&fixture.host, EventType::TaskFailed, &context),
        0
    );
    assert_eq!(
        run_records(&fixture.host, EventType::PolicyExecutionRecorded, &context),
        1,
        "one policy execution record"
    );
    fixture.assert_y_runs();
    fixture.close();
}

/// d (row B17): X's scheduled run succeeds; its release validated its lease, and before it takes
/// the admission guard the end grace runs out and a sweep tick ends the lease
/// (`ReleaseValidated`). Validated again under the guard, the release is the scheduler's
/// non-fatal denial ("lease already ended"), never a poison: the run's receipt fails non-fatal,
/// it settles once (a success, its sole terminal `task.completed`), nothing latches and Y's run
/// completes.
#[test]
fn iso5a_d_a_release_whose_lease_ends_before_its_guard_is_denied_not_poisoned() {
    let mut fixture = fixture(true);
    let (context, request) = fixture.run.take().expect("X's admitted run");
    let lease_id = context.lease_token().lease_id();
    let expiries = Arc::new(AtomicUsize::new(usize::MAX));
    let (hook_host, hook_clock, hook_expiries) = (
        Arc::clone(&fixture.host),
        Arc::clone(&fixture.clock),
        Arc::clone(&expiries),
    );
    let between = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::ReleaseValidated,
            fixture.x,
            lease_id,
            move |_| {
                hook_clock.advance(END_GRACE_MS + 1);
                hook_host
                    .expire_due_leases_for_test()
                    .expect("a sweep tick between the validation and the guard");
                hook_expiries.store(lease_expiries(&hook_host, lease_id), Ordering::SeqCst);
            },
        )
        .expect("arm the sweep between the release's validation and its guard");

    let error = fixture
        .host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the release meets the ended lease");

    assert_eq!(between.consumed(), 1, "the run's release was validated");
    assert_eq!(
        expiries.load(Ordering::SeqCst),
        1,
        "the sweep ended the lease before the guard"
    );
    assert!(!error.is_fatal(), "{error:?}");
    assert_runtime_runs(&fixture.host);
    assert_eq!(
        run_records(&fixture.host, EventType::TaskCompleted, &context),
        1
    );
    assert_eq!(
        run_records(&fixture.host, EventType::PolicyExecutionRecorded, &context),
        1,
        "one policy execution record"
    );
    fixture.assert_y_runs();
    fixture.close();
}

/// c (row B02): a client holds X's lease, and its input write is in flight when the lease lapses
/// (the clock passes the lease's end inside the write, which runs without the admission guard).
/// Three sweep ticks then meet the write: each leaves the lease, closes nothing and records
/// nothing (no `lease.expired`, no fence), and nothing latches. Once the write has returned, the
/// next tick ends the lease once. Y's run completes.
#[test]
fn iso5a_c_an_expiry_that_meets_a_write_in_flight_waits_for_it_and_records_nothing() {
    let fixture = fixture(false);
    let mut client = TestClient::connect(&fixture.host);
    let (_, token) = client.acquire(X_ALIAS);
    let lease_id = token.lease_id();
    let lapse_ms = token
        .expires_at_monotonic_ms()
        .saturating_sub(fixture.clock.monotonic_ms())
        + 1;
    let ticks = Arc::new(Mutex::new(Vec::new()));
    let (hook_host, hook_clock, hook_ticks) = (
        Arc::clone(&fixture.host),
        Arc::clone(&fixture.clock),
        Arc::clone(&ticks),
    );
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        hook_clock.advance(lapse_ms);
        for _ in 0..3 {
            let tick = hook_host.expire_due_leases_for_test();
            hook_ticks.lock().expect("ticks").push((
                tick.is_ok(),
                lease_expiries(&hook_host, lease_id),
                hook_host.fatal_error().expect("runtime health").is_none(),
            ));
        }
    });
    *fixture.x_state.input_hook.lock().expect("input hook") = Some(hook);

    let input = client.request(RuntimeOperation::Input {
        frame: None,
        token,
        action: InputAction::Reset,
    });
    let receipt = client.send(&input);

    assert_eq!(
        *ticks.lock().expect("ticks"),
        vec![(true, 0, true); 3],
        "each tick inside the write left the lease and latched nothing"
    );
    assert!(
        receipt
            .error_projection()
            .is_none_or(|projection| !projection.fatal),
        "{receipt:?}"
    );
    assert_runtime_runs(&fixture.host);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    fixture
        .host
        .expire_due_leases_for_test()
        .expect("a sweep tick after the write");
    assert_eq!(
        lease_expiries(&fixture.host, lease_id),
        1,
        "the first tick after the write ended the lease once"
    );
    assert_runtime_runs(&fixture.host);
    drop(client);
    fixture.assert_y_runs();
    fixture.close();
}
