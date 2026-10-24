// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A64 (v0.12.12; the 10-24 11:30 JST incident): a scheduled contained run reached its
//! deadline, and the close of its instance's resources began only after its lease (the deadline
//! plus the heartbeat reserve) had lapsed. `begin_resource_close` refused the lapsed lease
//! (`lease_expired`, operation `begin_destructive_resource_close`), the refusal was a poison
//! without terminal and the whole Runtime stopped. Requirements of the slice brief (R1, R3 (a)):
//! meeting the lapsed lease there never stops the Runtime; the run keeps its own terminal
//! (`contained_task_deadline_exceeded`); the instance's resources count as an unconfirmed close
//! of that instance only (the Workflow #403 fence, with the refusal as its cause), and the
//! refusal is recorded naming the lease and the stage; every other instance keeps working.
//!
//! The fixture is the incident's shape on fakes. Both registered instances capture through the
//! Nemu IPC, so every lease end closes the instance's session (the multi-Nemu gate,
//! `keeps_device_session`), as on the production host. The policy instance's scheduled run taps
//! and then waits for a page that never comes; the Runtime clock moves only when the kernel waits,
//! and the first wait after the tap reaches the run's deadline, so the run ends
//! `contained_task_deadline_exceeded` inside its reserve. At the run's lease end, after its
//! terminal and before its close, the clock moves on to the lease's expiry: the closeout begins
//! after the lease lapsed, as the incident's did. The lease sweep's expiry scan is held meanwhile,
//! so the run's own lease end, not the sweep, meets the lapsed lease. The instance's stuck
//! recovery is off, so its fence waits for a person instead of starting a ladder.
//!
//! Existing tests missed this because every lapse test (the #670 lapse points in
//! `instance_workers.rs`, the PK2 deadline tests in `gate_0122_pk1b.rs`) runs on instances that
//! keep their session across lease ends: their lease end closes nothing, so it never asks for the
//! close's write authority under the lapsed lease.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::all_events;
use super::*;
use crate::codes::HostLocation;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::{EffectiveDeviceConfiguration, InstanceStuckRecovery, RuntimePayload};
use std::sync::{Mutex, OnceLock};

const FENCED_ALIAS: &str = POLICY_INSTANCE_ALIAS;
const OTHER_ALIAS: &str = "a64.instance.other";
/// The scheduled run's response deadline.
const RESPONSE_DEADLINE_MS: u64 = 50_000;

/// Samples business capacity once and records it (`RuntimeHost::capacity_sampler_for_test`).
type CapacitySampler = Box<dyn Fn() -> RuntimeHostResult<()> + Send>;

/// The Runtime clock of this test. It moves only when the contained-task kernel waits, by the
/// wait's length rounded up to the millisecond (test plan E-3, §3.10), or when the test moves it.
/// Once armed with the run's deadline, the first wait after the fenced instance's tap moves it to
/// that deadline instead: the run has waited for its page until its deadline. Every move samples
/// business capacity at the moved time, as the capacity monitor does while time passes (the
/// `gate_0122_pk1b.rs` `WaitClock` fix: a sample is fresh for 4 s of this clock, and the run's
/// frame admissions read it).
struct DeadlineClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
    /// The run's deadline once armed; 0 before that and after the move to it.
    deadline_ms: AtomicU64,
    /// The fenced instance's device: the jump waits for its tap.
    device: Arc<FakeState>,
    sampler: OnceLock<Mutex<CapacitySampler>>,
}

impl DeadlineClock {
    fn new(unix_ms: u64, device: Arc<FakeState>) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
            deadline_ms: AtomicU64::new(0),
            device,
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

    /// The first wait after the fenced instance's tap moves the clock to `deadline_ms`.
    fn arm_deadline(&self, deadline_ms: u64) {
        self.deadline_ms.store(deadline_ms, Ordering::SeqCst);
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

impl RuntimeClock for DeadlineClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.load(Ordering::SeqCst),
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }

    fn pause(&self, duration: Duration) {
        let wait_ms = u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX);
        let now = self.monotonic_ms();
        let deadline_ms = self.deadline_ms.load(Ordering::SeqCst);
        let tapped = self.device.input_count.load(Ordering::Acquire) > 0;
        if tapped && deadline_ms > now {
            self.deadline_ms.store(0, Ordering::SeqCst);
            self.advance(deadline_ms - now);
        } else {
            self.advance(wait_ms);
        }
    }
}

/// A device configuration whose capture is the Nemu IPC (neutral test values).
fn nemu_capture(serial: &str) -> EffectiveDeviceConfiguration {
    EffectiveDeviceConfiguration {
        input_backend: "adb_shell_input".to_owned(),
        capture_backend: "nemu_ipc".to_owned(),
        input_adb: "<sealed-test>".to_owned(),
        capture_adb: "<sealed-test>".to_owned(),
        configured_serial: None,
        resolved_serial: serial.to_owned(),
        input_command_timeout_ms: 5_000,
        capture_command_timeout_ms: 5_000,
        capture_timeout_ms: 5_000,
        configured_mumu_root: None,
        configured_capture_dll: None,
    }
}

/// The code of the run's committed terminal outcome, one per terminal among `records`.
fn terminal_codes(records: &[PersistedEvent]) -> Vec<Option<String>> {
    records
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::TerminalCommitted { outcome, .. } => Some(
                    outcome
                        .as_ref()
                        .map(|outcome| outcome.code().as_str().to_owned()),
                ),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// R1, R3 (a): the policy instance's scheduled run reaches its deadline and its resource close
/// begins after its lease's expiry. The Runtime still runs (no fatal state); the run ends with its
/// own terminal, `contained_task_deadline_exceeded`, which the host also returns; the instance is
/// fenced with the refusal `lease_expired` as its cause (in the status and in its one fence record,
/// which names the instance), and the refusal is recorded once, naming the run's lease and the
/// stage `runtime_lifecycle_session_close`; the other instance is not fenced and its direct run
/// completes.
#[test]
fn a64_a_closeout_after_the_lease_lapsed_fences_only_its_instance() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("a64-deadline-task.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write package");
    // The fenced instance shows `home` before and after its tap: the run's page never comes.
    let fenced_state = Arc::new(FakeState::default());
    fenced_state
        .physical_task_geometry
        .store(true, Ordering::Release);
    let other_state = Arc::new(FakeState::default());
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let fenced = instance_id();
    let other = instance_id();
    // The policy's configured identities are the registered aliases; only the fenced instance is
    // available to it.
    let mut facts = policy_facts();
    let mut other_identity = facts.instances[0].clone();
    other_identity.instance_id = OTHER_ALIAS.to_owned();
    other_identity.available = false;
    facts.instances.push(other_identity);
    let provider = FakeProvider::from_entries([
        (FENCED_ALIAS.to_owned(), fenced, Arc::clone(&fenced_state)),
        (OTHER_ALIAS.to_owned(), other, Arc::clone(&other_state)),
    ]);
    let fenced_resolved = provider
        .resolve(FENCED_ALIAS)
        .expect("the fenced instance")
        .with_configuration(nemu_capture("TESTSERIAL0001"));
    let other_resolved = provider
        .resolve(OTHER_ALIAS)
        .expect("the other instance")
        .with_configuration(nemu_capture("TESTSERIAL0002"));
    let provider = provider
        .with_resolved_for(FENCED_ALIAS, fenced_resolved)
        .with_resolved_for(OTHER_ALIAS, other_resolved);
    let clock = Arc::new(DeadlineClock::new(
        POLICY_NOW_UNIX_MS,
        Arc::clone(&fenced_state),
    ));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), policy_resources()))
            .with_scheduled_procedures(["procedure.observe".to_owned()])
            .with_stuck_recovery(BTreeMap::from([(
                FENCED_ALIAS.to_owned(),
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
        .find(|intent| intent.instance_id == FENCED_ALIAS)
        .expect("the fenced instance's dispatch intent")
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("dispatch reason chain")
        .clone();
    record_policy_approval(&host, &intent);
    let request = ContainedTaskRequest::new(package.display().to_string())
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
    let lease = context.lease_token().clone();
    let deadline_ms = clock.monotonic_ms() + RESPONSE_DEADLINE_MS;
    let expires_at = lease.expires_at_monotonic_ms();
    assert!(
        expires_at > deadline_ms,
        "the lease lasts its reserve past the run's deadline"
    );
    clock.arm_deadline(deadline_ms);
    // At the run's lease end, after its terminal and before its close, the clock reaches the
    // lease's expiry.
    let lease_end_clock = Arc::clone(&clock);
    let lease_end = host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::FailedRunLeaseEnd,
            fenced,
            lease.lease_id(),
            move |_| {
                let now = lease_end_clock.monotonic_ms();
                lease_end_clock.advance(expires_at.saturating_sub(now));
            },
        )
        .expect("arm the late closeout");
    let scan = host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the run reaches its deadline");

    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "a closeout that met its lapsed lease ended the Runtime: {error:?}"
    );
    assert_eq!(lease_end.consumed(), 1, "the run reached its lease end");
    assert!(
        clock.monotonic_ms() >= expires_at,
        "the close began after the lease's expiry"
    );
    let deadline_exceeded = RuntimeErrorCode::ContainedTaskDeadlineExceeded.as_str();
    assert!(!error.is_fatal(), "{error:?}");
    assert_eq!(error.code(), deadline_exceeded, "{error:?}");
    let run_records = host
        .query_persisted_events_for_test(EventQuery {
            run_id: Some(context.run_id()),
            ..EventQuery::default()
        })
        .expect("the run's records");
    assert_eq!(
        run_records
            .iter()
            .filter(|event| event.event_type() == EventType::TaskFailed)
            .count(),
        1,
        "one failed terminal"
    );
    assert_eq!(
        terminal_codes(&run_records),
        vec![Some(deadline_exceeded.to_owned())],
        "the run keeps its own terminal"
    );
    let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
    let fence = status_fence(&host, FENCED_ALIAS).expect("the instance's fence in the status");
    assert_eq!(fence.cause.as_str(), lease_expired, "{fence:?}");
    assert_eq!(status_fence(&host, OTHER_ALIAS), None);
    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(fence_record, fence_outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(fence_record.links().instance_id(), Some(&fenced));
    assert_fence(fence_outcome, fenced, lease_expired);
    let refusals = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.outcome().code().as_str() == lease_expired =>
            {
                Some(record.outcome())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [refusal] = refusals.as_slice() else {
        panic!("one record of the refusal: {refusals:#?}");
    };
    assert_eq!(
        refusal.value("lease_id"),
        Some(&serde_json::to_value(lease.lease_id()).expect("lease id")),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.value("stage").and_then(serde_json::Value::as_str),
        Some(HostLocation::RuntimeLifecycleSessionClose.as_str()),
        "{refusal:?}"
    );
    drop(scan);

    // Every other instance keeps working.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let other_run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            OTHER_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(other_package).expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&other_run, ConnectionId::new(641).expect("connection"))
        .expect("the other instance's run receipt");
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
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
