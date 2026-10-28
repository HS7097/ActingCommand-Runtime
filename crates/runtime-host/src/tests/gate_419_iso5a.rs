// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-5a (Workflow #419; ISO-M rows A08, A11, B02, B17; coordinator ruling C-43 point 3): no step
//! of a run's end, or of a lease holder's end, may stop the Runtime.
//!
//! - A run that ends otherwise than at its deadline (here a success) enters its end phase too:
//!   while it holds its lease, its lease is renewed once by the end grace
//!   (`host_claims.run_end_grace_ms`, default 120 000 ms) when it would lapse inside the grace, and
//!   its terminal and its release run under that lease. A success whose end work passes the
//!   lease's original end settles once as a success; nothing stops (rows A08, A11).
//! - A success whose lease the sweep ended after the end grace still settles once, as a success
//!   (its sole terminal is `task.completed`), never as `policy_run_failure_terminal_conflict`;
//!   only the run's receipt fails, non-fatal (row A11). Fix 1 (review M1): it settles as the
//!   success path settles it, so its authoritative outcome is published, and the next evaluation
//!   round projects it and goes on scheduling every instance.
//! - A lease expiry that meets a write still in flight on the lease (an input writes without the
//!   admission guard) closes nothing and records nothing; the sweep retries it (row B02).
//!
//! Fix 1 (review M2): the instances X (the policy instance) and Y both capture through the Nemu
//! IPC, as the production host's MuMu instances do, so every lease end closes the instance's
//! session (the multi-Nemu gate, `keeps_device_session`; the `gate_411_a64.rs` fixture). Each
//! test reaches that close at its lease end. The Runtime clock moves only when the kernel waits or
//! the test moves it; every move samples business capacity, as the capacity monitor does while
//! time passes (the `gate_411_a64.rs` clock). No sleep, no wall clock.
//!
//! The settlement test (e) runs on the two-instance mapped-outcome fixture (`gate_ln_ws_f.rs`
//! T7): a catalog whose producers' outcome keys a follow-up references, so every evaluation
//! projects the producers' authoritative outcomes.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::gate_411_a64::nemu_capture;
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
/// Test (e): the outcome key the producers' package maps its designated effect to.
const MAPPED_KEY: &str = "iso5a-success-settled";

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

    fn unix_ms(&self) -> u64 {
        self.unix_ms.load(Ordering::SeqCst)
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

/// The started host with X and Y behind the multi-Nemu gate; with `admit`, X's scheduled run is
/// admitted.
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
    // Fix 1 (review M2): both instances capture through the Nemu IPC, so every lease end closes
    // the instance's session, as on the production host (the multi-Nemu gate).
    let x_resolved = provider
        .resolve(X_ALIAS)
        .expect("X")
        .with_configuration(nemu_capture("TESTSERIAL0001"));
    let y_resolved = provider
        .resolve(Y_ALIAS)
        .expect("Y")
        .with_configuration(nemu_capture("TESTSERIAL0002"));
    let provider = provider
        .with_resolved_for(X_ALIAS, x_resolved)
        .with_resolved_for(Y_ALIAS, y_resolved);
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

    /// The close at X's lease end met X's lapsed lease (`lease_expired`): only X is fenced, with
    /// that cause, in the status and in its one fence record, which names it (#411 A64, the
    /// multi-Nemu close; ruling C-43).
    fn assert_x_fenced_by_the_lapse(&self) {
        let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
        let fence = status_fence(&self.host, X_ALIAS).expect("X's fence");
        assert_eq!(fence.cause.as_str(), lease_expired, "{fence:?}");
        let events = all_events(&self.host);
        let fences = fence_records(&events);
        let [(fence_record, fence_outcome)] = fences.as_slice() else {
            panic!("one fence record: {fences:#?}");
        };
        assert_eq!(fence_record.links().instance_id(), Some(&self.x));
        assert_fence(fence_outcome, self.x, lease_expired);
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
/// completes, its release closes X's session (the multi-Nemu close at its lease end), it settles
/// once as a success (one `policy.execution_recorded`), its lease is released once and never
/// expired, nothing latches, X is not fenced and Y's run completes.
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
    assert!(
        fixture.x_state.close_count.load(Ordering::Acquire) > 0,
        "the run's lease end closed X's session (the multi-Nemu gate)"
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
/// end work has run past the end grace: a sweep tick there ends its lease, and that expiry's close
/// of X's session (the multi-Nemu close at the lease end) meets the lapsed lease and fences only X
/// (`lease_expired`). The run's success terminal still comes, and its release meets the ended
/// lease. The run's sole terminal is `task.completed`, so it settles once as a success (never
/// `policy_run_failure_terminal_conflict`); only its receipt fails, non-fatal; nothing latches
/// and Y's run completes.
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
    fixture.assert_x_fenced_by_the_lapse();
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
/// (`ReleaseValidated`); that expiry's close of X's session meets the lapsed lease and fences
/// only X (`lease_expired`). Validated again under the guard, the release is the scheduler's
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
    fixture.assert_x_fenced_by_the_lapse();
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

/// c (row B02, fix 1, review M2): X's scheduled run taps, and while the tap's write is in flight
/// (an input writes without the admission guard) the clock passes the run's lease end, and with
/// it the run's deadline. Three sweep ticks then meet the write: each leaves the lease, closes
/// nothing and records nothing (no `lease.expired`, no fence), and nothing latches. The lease
/// sweep is then held, and the write returns. The run's next capture finds its deadline passed;
/// its end phase, entered at that deadline, keeps the lapsed lease (its renewal is refused), and
/// the close at its lease end (the multi-Nemu close) meets the lapsed lease: only X is fenced
/// (`lease_expired`), the lease is ended by the run itself and never expired, the run returns
/// its own `contained_task_deadline_exceeded`, non-fatal, and Y's run completes.
///
/// Before the fix, the sweep's expiry closed X's session before it looked at the write
/// (`end_lease_device_use` closed first on the multi-Nemu gate), and `begin_resource_close`
/// refused the lapsed lease (`lease_expired`) of a run not yet in its end phase: the first tick
/// failed.
#[test]
fn iso5a_c_an_expiry_that_meets_a_write_in_flight_waits_for_it_and_records_nothing() {
    let mut fixture = fixture(true);
    let (context, request) = fixture.run.take().expect("X's admitted run");
    let lease = context.lease_token().clone();
    let lease_id = lease.lease_id();
    // The tap's write reports `true` once it is in flight; the run reports `false` once it ended.
    let (report, reports) = mpsc::channel::<bool>();
    let (let_write_return, write_returns) = mpsc::channel::<()>();
    let write_report = report.clone();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        write_report
            .send(true)
            .expect("the tap's write is in flight");
        write_returns.recv().expect("the tap's write returns");
    });
    *fixture.x_state.input_hook.lock().expect("input hook") = Some(hook);

    let (host, run_context, run_request) = (&fixture.host, &context, &request);
    let (ticks, result) = thread::scope(|scope| {
        let run = scope.spawn(move || {
            let result = host.run_scheduled_contained_task(run_context, run_request);
            report.send(false).expect("X's run ended");
            result
        });
        assert!(
            reports.recv().expect("X's run reports"),
            "X's run ended before its tap reached its write"
        );
        fixture.clock.advance(
            lease
                .expires_at_monotonic_ms()
                .saturating_sub(fixture.clock.monotonic_ms())
                + 1,
        );
        let ticks = (0..3)
            .map(|_| {
                let tick = fixture.host.expire_due_leases_for_test();
                (
                    tick.is_ok(),
                    lease_expiries(&fixture.host, lease_id),
                    fixture
                        .host
                        .fatal_error()
                        .expect("runtime health")
                        .is_none(),
                )
            })
            .collect::<Vec<_>>();
        // The run's own end meets its lapsed lease from here; the sweep waits for it.
        let scan = fixture
            .host
            .hold_lease_expiry_scan_for_test()
            .expect("hold the lease sweep's expiry scan");
        let_write_return
            .send(())
            .expect("let the tap's write return");
        let result = run.join().expect("X's run thread");
        drop(scan);
        (ticks, result)
    });

    assert_eq!(
        ticks,
        vec![(true, 0, true); 3],
        "each tick inside the write left the lease, recorded nothing and latched nothing"
    );
    let error = result.expect_err("the run passed its deadline inside the write");
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(
        error.code(),
        RuntimeErrorCode::ContainedTaskDeadlineExceeded.as_str(),
        "{error:?}"
    );
    assert_runtime_runs(&fixture.host);
    assert_eq!(
        lease_expiries(&fixture.host, lease_id),
        0,
        "the run's own end ended its lease; the sweep never expired it"
    );
    fixture.assert_x_fenced_by_the_lapse();
    fixture.assert_y_runs();
    fixture.close();
}

/// Test (e): the mapped facts with instance B, available or not (`gate_ln_ws_f.rs` T7's
/// `with_instance_b`). Once B is available, X's follow-up is stopped (its `fixture.followup.stop`
/// fact), so the round's dispatches are the producers'.
fn mapped_facts(instance_b_available: bool, snapshot: &str) -> EvaluationFacts {
    let mut facts = mapped_policy_facts(MAPPED_KEY, false);
    facts.fact_snapshot_id = snapshot.to_owned();
    for fact in &mut facts.facts {
        if fact.fact_key == "fixture.followup.stop" {
            fact.value = FactValue::Boolean(instance_b_available);
        }
    }
    facts.instances.push(InstanceSnapshot {
        instance_id: POLICY_INSTANCE_ALIAS_B.to_owned(),
        server_id: "fixture-server-b".to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-b".to_owned(),
        available: instance_b_available,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    facts
}

/// Test (e): the mapped catalog with a copy of its producer on instance B (`fixture.observe-b`),
/// as `gate_ln_ws_f.rs` T7 builds it.
fn mapped_sources_with_instance_b() -> CatalogSources {
    let mut sources = mapped_policy_sources(1, MAPPED_KEY);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("the mapped tasks");
    let mut producer_b = tasks["tasks"][0].clone();
    producer_b["id"] = serde_json::json!("fixture.observe-b");
    producer_b["scope"] =
        serde_json::json!({"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS_B});
    producer_b["procedure_ref"] = serde_json::json!("procedure.observe-b");
    producer_b["produces"] = serde_json::json!([]);
    producer_b["instance_overrides"] = serde_json::json!([]);
    tasks["tasks"]
        .as_array_mut()
        .expect("the mapped tasks array")
        .push(producer_b);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("the mapped task bytes");
    sources
}

/// e (row A11; fix 1, review M1): a catalog whose follow-up references the producer's outcome
/// keys, so every evaluation projects the producer's authoritative outcome. X's producer run
/// succeeds (its terminal maps the designated effect to `MAPPED_KEY`), and at
/// `BeforeTerminalAppend` its end work has run past the end grace: a sweep tick ends its lease,
/// and its release meets the ended lease. It settles once as a success through the failure path,
/// which publishes its outcome as the success path does. The next evaluation round, with instance
/// B available, projects every instance's outcomes without `outcome_projection_not_ready` and
/// schedules B's producer: the other instance keeps being scheduled, and nothing latches.
#[test]
fn iso5a_e_a_success_settled_after_its_lease_ended_publishes_its_outcome_for_the_next_round() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_mapped_contained_task_package(MAPPED_KEY, "designated_effect_completed");
    let package_path = root.path().join("iso5a-mapped-task.zip");
    fs::write(&package_path, &package).expect("write the mapped package");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the mapped package request");
    let x = instance_id();
    let entries = [
        (POLICY_INSTANCE_ALIAS, x),
        (POLICY_INSTANCE_ALIAS_B, instance_id()),
    ]
    .map(|(alias, id)| {
        let state = Arc::new(FakeState::default());
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        (alias.to_owned(), id, state)
    });
    let clock = Arc::new(SampledClock::new(POLICY_NOW_UNIX_MS));
    let facts = mapped_facts(false, "snapshot:iso5a:first-round");
    let host = Arc::new(
        RuntimeHost::start(
            config(&root)
                .with_runtime_clock(clock.clone())
                .with_policy_inputs(PolicyInputSnapshot::new(
                    facts.clone(),
                    pending_policy_resources(),
                ))
                .with_procedure_manifest(procedure_manifest_with_primary(
                    &package,
                    vec!["after_observation".to_owned()],
                )),
            Arc::new(FakeProvider::from_entries(entries).fixture_simulation()),
        )
        .expect("the two-instance host"),
    );
    clock.sample_capacity_after_moves(host.capacity_sampler_for_test().expect("capacity owner"));
    clock.advance(0);
    host.activate_policy_catalog(&mapped_sources_with_instance_b())
        .expect("activate the catalog");
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &facts,
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("the first round");
    let evaluation = cycle
        .evaluation
        .as_ref()
        .expect("the first round's evaluation");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == "fixture.observe" && intent.instance_id == X_ALIAS)
        .unwrap_or_else(|| panic!("X's producer is dispatched: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the dispatch's reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("X's producer is admitted")
    else {
        panic!("expected a granted dispatch of X's producer")
    };
    let lease_id = context.lease_token().lease_id();
    let expiries = Arc::new(AtomicUsize::new(usize::MAX));
    let (hook_host, hook_clock, hook_expiries) =
        (Arc::clone(&host), Arc::clone(&clock), Arc::clone(&expiries));
    let past_grace = host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::BeforeTerminalAppend,
            x,
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

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the release meets the ended lease");

    assert_eq!(past_grace.consumed(), 1, "the run reached its terminal");
    assert_eq!(
        expiries.load(Ordering::SeqCst),
        1,
        "the sweep ended the lease after the end grace"
    );
    assert!(!error.is_fatal(), "{error:?}");
    assert_runtime_runs(&host);
    assert_eq!(run_records(&host, EventType::TaskCompleted, &context), 1);
    assert_eq!(run_records(&host, EventType::TaskFailed, &context), 0);
    assert_eq!(
        run_records(&host, EventType::PolicyExecutionRecorded, &context),
        1,
        "one policy execution record, a success"
    );

    let now = clock.unix_ms();
    let next = host
        .evaluate_policy_cycle_with_test_inputs(
            &mapped_facts(true, "snapshot:iso5a:next-round"),
            &pending_policy_resources(),
            EvaluationTime {
                unix_ms: now,
                monotonic_ms: now,
            },
            8,
            PolicyTrigger::FactsChanged,
        )
        .unwrap_or_else(|error| {
            panic!("the next round projects every instance's outcomes: {error:?}")
        });
    let next_evaluation = next
        .evaluation
        .as_ref()
        .expect("the next round's evaluation");
    assert!(
        next_evaluation.dispatch_intents.iter().any(|intent| {
            intent.task_id == "fixture.observe-b" && intent.instance_id == POLICY_INSTANCE_ALIAS_B
        }),
        "the other instance keeps being scheduled: {next_evaluation:#?}"
    );
    assert_runtime_runs(&host);
    drop(past_grace);
    let Ok(host) = Arc::try_unwrap(host) else {
        panic!("a hook still holds the host");
    };
    host.close().expect("close host");
}
