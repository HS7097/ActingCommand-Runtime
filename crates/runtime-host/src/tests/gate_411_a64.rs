// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A64 (v0.12.12; the 10-24 11:30 JST incident): a scheduled contained run reached its
//! deadline, and the close of its instance's resources began only after its lease (the deadline
//! plus the heartbeat reserve) had lapsed. `begin_resource_close` refused the lapsed lease
//! (`lease_expired`, operation `begin_destructive_resource_close`), the refusal was a poison
//! without terminal and the whole Runtime stopped.
//!
//! Requirements (slice brief R1, R3; coordinator ruling C-43 of 10-24):
//! - a run that reaches its deadline enters an end phase: before any end work its lease is
//!   renewed once by the end grace (`host_claims.run_end_grace_ms`, default 120 000 ms), and every
//!   step of its end runs under that lease;
//! - the lease sweep never expires the lease of a run inside its end grace; after the grace it
//!   may;
//! - no lapsed lease anywhere in a run's end stops the Runtime: a close that meets its lapsed
//!   lease fences only that instance (cause `lease_expired`, the refusal recorded naming the
//!   lease and the stage); a terminal that finds its lease already expired by the sweep is still
//!   recorded, carrying `lease_expired`, and the instance is fenced; every other instance keeps
//!   working.
//!
//! The fixture is the incident's shape on fakes, on the shipped scheduler (a heartbeat reserve of
//! 5 s). Both registered instances capture through the Nemu IPC, so every lease end closes the
//! instance's session (the multi-Nemu gate, `keeps_device_session`), as on the production host.
//! The policy instance's scheduled run taps and then waits for a page that never comes
//! (`deadline_package`: its step timeout, task timeout and step budget all outlast the deadline);
//! the Runtime clock moves only when the kernel waits, and the first wait after the tap reaches the
//! run's deadline, so the run ends `contained_task_deadline_exceeded`. A checkpoint hook then
//! moves the clock on inside the run's end: at its lease end (`FailedRunLeaseEnd`, after its
//! terminal and before its close) or at the start of its end work (`EndWork`, before the diagnostic
//! seal, the capture summary and the terminal). The instance's stuck recovery is off, so its fence
//! waits for a person instead of starting a ladder.
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
/// Ruling C-43: the end grace in effect, the default of `host_claims.run_end_grace_ms`.
const END_GRACE_MS: u64 = 120_000;
/// Test (c): end work longer than the old reserve (5 s) and shorter than the end grace.
const LONG_END_WORK_MS: u64 = 60_000;

/// Samples business capacity once and records it (`RuntimeHost::capacity_sampler_for_test`).
type CapacitySampler = Box<dyn Fn() -> RuntimeHostResult<()> + Send>;

/// The Runtime clock of these tests. It moves only when the contained-task kernel waits, by the
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

/// The scheduled run's package: one tap from `home` that waits for `terminal`, which never comes
/// on a device that stays on `home`. The tap's confirmation captures a frame every 5 s; its step
/// timeout (60 s) and its task timeout (120 s) both run past the run's deadline (50 s), and its
/// step budget (2) is never reached, so the first wait after the tap (which `DeadlineClock` moves
/// to the deadline) is followed by a capture that ends the run `contained_task_deadline_exceeded`.
/// The frame is the fenced instance's physical 16x9 extent (`physical_task_geometry`).
fn deadline_package() -> Vec<u8> {
    let target = |id: &str, expected: [u8; 3]| {
        serde_json::json!({
            "type": "color",
            "id": id,
            "region": {"x": 0, "y": 0, "width": 1, "height": 1},
            "expected": expected
        })
    };
    let page = |id: &str, required: &str| {
        serde_json::json!({
            "id": id,
            "required": [required],
            "optional": [],
            "forbidden": []
        })
    };
    let files = [
        (
            "control.json",
            serde_json::json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": "neutral.a64.deadline-task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 16, "height": 9},
                "entry_task_id": "task",
                "capture_interval_ms": 5_000,
                "step_timeout_ms": 60_000,
                "timeout_ms": 120_000,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            serde_json::json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 16, "height": 9},
                "entry_page": "home",
                "target_page": "terminal",
                "operations": [{
                    "id": "open_terminal",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            serde_json::json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 16, "height": 9},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    target("page/home", [255, 0, 0]),
                    target("page/terminal", [0, 0, 255])
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            serde_json::json!({
                "schema_version": "0.3",
                "pages": [
                    page("neutral/home", "page/home"),
                    page("neutral/terminal", "page/terminal")
                ]
            }),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// A device configuration whose capture is the Nemu IPC (neutral test values). ISO-5a
/// (`gate_419_iso5a.rs`) puts its instances behind the same multi-Nemu gate with it.
pub(super) fn nemu_capture(serial: &str) -> EffectiveDeviceConfiguration {
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

/// The started host with both instances and the fenced instance's scheduled run admitted.
struct Fixture {
    _root: TempDir,
    host: Arc<RuntimeHost>,
    clock: Arc<DeadlineClock>,
    fenced: InstanceId,
    fenced_state: Arc<FakeState>,
    other_package: String,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
    /// The run's lease as its admission granted it.
    lease: LeaseToken,
    /// The run's deadline on the Runtime clock.
    deadline_ms: u64,
}

/// Starts the policy instance (the fenced one) and the other instance, both capturing through
/// the Nemu IPC, on the shipped scheduler; admits the fenced instance's scheduled run, whose page
/// never comes, and arms the clock with its deadline.
fn fixture() -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("a64-deadline-task.zip");
    fs::write(&package, deadline_package()).expect("write package");
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
            .with_scheduler(SchedulerConfig::default())
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
    assert_eq!(
        lease.expires_at_monotonic_ms(),
        deadline_ms + reserve_ms(),
        "the admitted lease lasts the reserve past the run's deadline"
    );
    clock.arm_deadline(deadline_ms);
    Fixture {
        _root: root,
        host: Arc::new(host),
        clock,
        fenced,
        fenced_state,
        other_package,
        context,
        request,
        lease,
        deadline_ms,
    }
}

/// The shipped heartbeat reserve: how long an admitted run's lease lasts past its deadline.
fn reserve_ms() -> u64 {
    SchedulerConfig::default().maximum_client_heartbeat_interval_ms
}

impl Fixture {
    /// Runs the scheduled run, which reaches its deadline: the host returns its failure.
    fn run_to_deadline(&self) -> RuntimeHostError {
        self.host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("the run reaches its deadline")
    }

    /// The Runtime still runs, and the host returned the run's own failure.
    fn assert_runtime_runs_and_returns_the_deadline(&self, error: &RuntimeHostError) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "a lapsed lease in the run's end ended the Runtime: {error:?}"
        );
        assert!(!error.is_fatal(), "{error:?}");
        assert_eq!(
            error.code(),
            RuntimeErrorCode::ContainedTaskDeadlineExceeded.as_str(),
            "{error:?}"
        );
    }

    /// The run's records.
    fn run_records(&self) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(self.context.run_id()),
                ..EventQuery::default()
            })
            .expect("the run's records")
    }

    /// The run has one failed terminal whose outcome is `contained_task_deadline_exceeded`;
    /// returns the terminal's record and every code its outcome carries.
    fn deadline_terminal(&self) -> (PersistedEvent, Vec<String>) {
        let records = self.run_records();
        assert_eq!(
            records
                .iter()
                .filter(|event| event.event_type() == EventType::TaskFailed)
                .count(),
            1,
            "one failed terminal"
        );
        let terminals = records
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted { outcome, .. } => {
                        Some((event.clone(), outcome.clone()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(terminal, Some(outcome))] = terminals.as_slice() else {
            panic!("one committed terminal with an outcome: {terminals:#?}");
        };
        assert_eq!(
            outcome.code().as_str(),
            RuntimeErrorCode::ContainedTaskDeadlineExceeded.as_str(),
            "the run keeps its own terminal: {outcome:?}"
        );
        let codes = outcome
            .codes()
            .iter()
            .map(|code| code.as_str().to_owned())
            .collect();
        (terminal.clone(), codes)
    }

    /// The run's lease was renewed once, by the end grace, before its terminal.
    fn assert_renewed_once_before(&self, terminal: &PersistedEvent) {
        let renewals = all_events(&self.host)
            .into_iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseRenewed
                    && event.links().lease_id() == Some(&self.lease.lease_id())
            })
            .collect::<Vec<_>>();
        let [renewal] = renewals.as_slice() else {
            panic!("one renewal of the run's lease: {renewals:#?}");
        };
        assert!(
            renewal.sequence() < terminal.sequence(),
            "the end grace precedes the terminal"
        );
    }

    /// The `lease.expired` records of the run's lease.
    fn lease_expiries(&self) -> usize {
        lease_expiries(&self.host, self.lease.lease_id())
    }

    /// The other instance is not fenced, and its direct run is dispatched and completes.
    fn assert_other_instance_runs(&self) {
        assert_eq!(status_fence(&self.host, OTHER_ALIAS), None);
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let other_run = runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                OTHER_ALIAS,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.other_package.clone()).expect("task request"),
            ),
        );
        let receipt = self
            .host
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
        assert!(self.host.fatal_error().expect("runtime health").is_none());
    }

    /// Closes the host once every hook that shared it has run.
    fn close(self) {
        let Ok(host) = Arc::try_unwrap(self.host) else {
            panic!("a hook still holds the host");
        };
        host.close().expect("close host");
    }
}

/// The `lease.expired` records of `lease_id`.
fn lease_expiries(host: &RuntimeHost, lease_id: LeaseId) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::LeaseExpired),
        lease_id: Some(lease_id),
        ..EventQuery::default()
    })
    .expect("the lease's expiries")
    .len()
}

/// The instance is fenced with `lease_expired` as its cause, in the status and in its one fence
/// record, which names it.
fn assert_fenced_by_the_lapse(fixture: &Fixture) {
    let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
    let fence = status_fence(&fixture.host, FENCED_ALIAS).expect("the instance's fence");
    assert_eq!(fence.cause.as_str(), lease_expired, "{fence:?}");
    let events = all_events(&fixture.host);
    let fences = fence_records(&events);
    let [(fence_record, fence_outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(fence_record.links().instance_id(), Some(&fixture.fenced));
    assert_fence(fence_outcome, fixture.fenced, lease_expired);
}

/// R1, R3 (a), adapted by ruling C-43 (the end grace now covers the end; this test keeps the end
/// that overruns it): the run reaches its deadline, its lease is renewed once by the end grace
/// before its terminal, and its resource close at its lease end begins after the grace ended (the
/// lease sweep's expiry scan is held, so the run's own lease end meets the lapsed lease). The
/// Runtime still runs; the run keeps its own terminal, `contained_task_deadline_exceeded`, which
/// the host also returns; the instance is fenced with the refusal `lease_expired` as its cause,
/// and the refusal is recorded once, naming the run's lease and the stage
/// `runtime_lifecycle_session_close`; the other instance is not fenced and its direct run
/// completes.
#[test]
fn a64_a_closeout_after_the_lease_lapsed_fences_only_its_instance() {
    let fixture = fixture();
    // At the run's lease end, after its terminal and before its close, the end grace runs out.
    let lease_end_clock = Arc::clone(&fixture.clock);
    let lease_end = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::FailedRunLeaseEnd,
            fixture.fenced,
            fixture.lease.lease_id(),
            move |_| lease_end_clock.advance(END_GRACE_MS + 1),
        )
        .expect("arm the late closeout");
    let scan = fixture
        .host
        .hold_lease_expiry_scan_for_test()
        .expect("hold the lease sweep's expiry scan");

    let error = fixture.run_to_deadline();

    fixture.assert_runtime_runs_and_returns_the_deadline(&error);
    assert_eq!(lease_end.consumed(), 1, "the run reached its lease end");
    assert!(
        fixture.clock.monotonic_ms() > fixture.deadline_ms + END_GRACE_MS,
        "the close began after the end grace"
    );
    let (terminal, _) = fixture.deadline_terminal();
    fixture.assert_renewed_once_before(&terminal);
    assert_fenced_by_the_lapse(&fixture);
    let lease_expired = RuntimeErrorCode::LeaseExpired.as_str();
    let events = all_events(&fixture.host);
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
        Some(&serde_json::to_value(fixture.lease.lease_id()).expect("lease id")),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.value("stage").and_then(serde_json::Value::as_str),
        Some(HostLocation::RuntimeLifecycleSessionClose.as_str()),
        "{refusal:?}"
    );
    drop(scan);

    fixture.assert_other_instance_runs();
    fixture.close();
}

/// Ruling C-43 test (c): the run reaches its deadline and its end work takes longer than the old
/// reserve (5 s) but less than the end grace (here 60 s, from the start of its end work, before
/// its diagnostic seal, capture summary and terminal). The lease sweep runs as it does. The run
/// ends normally: the Runtime runs, its one terminal is `contained_task_deadline_exceeded` with
/// no lapse, its lease was renewed once before that terminal, never expired and released once,
/// its instance's session was closed, and no instance is fenced.
#[test]
fn a64_c_end_work_longer_than_the_reserve_and_inside_the_grace_ends_normally() {
    let fixture = fixture();
    let end_work_clock = Arc::clone(&fixture.clock);
    let end_work = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            fixture.fenced,
            fixture.lease.lease_id(),
            move |_| end_work_clock.advance(LONG_END_WORK_MS),
        )
        .expect("arm the long end work");
    assert!(LONG_END_WORK_MS > reserve_ms() && LONG_END_WORK_MS < END_GRACE_MS);

    let error = fixture.run_to_deadline();

    fixture.assert_runtime_runs_and_returns_the_deadline(&error);
    assert_eq!(end_work.consumed(), 1, "the run's end work ran");
    let (terminal, codes) = fixture.deadline_terminal();
    assert!(
        !codes
            .iter()
            .any(|code| code == RuntimeErrorCode::LeaseExpired.as_str()),
        "no lapse: {codes:?}"
    );
    fixture.assert_renewed_once_before(&terminal);
    assert_eq!(fixture.lease_expiries(), 0, "the lease never lapsed");
    assert_eq!(
        all_events(&fixture.host)
            .iter()
            .filter(|event| {
                event.event_type() == EventType::LeaseReleased
                    && event.links().lease_id() == Some(&fixture.lease.lease_id())
            })
            .count(),
        1,
        "the run released its lease once"
    );
    assert!(
        fixture.fenced_state.close_count.load(Ordering::Acquire) > 0,
        "the lease end closed the instance's session"
    );
    assert_eq!(status_fence(&fixture.host, FENCED_ALIAS), None);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    fixture.close();
}

/// Ruling C-43 test (d), the 10-24 order: the run reaches its deadline and is still at its end
/// work (before its diagnostic seal, capture summary and terminal) when a lease sweep tick comes
/// after the deadline plus the old reserve: inside the end grace the sweep leaves the lease alone.
/// The end work then runs past the grace and the next tick expires the lease (its expiry end
/// fences the instance). The run's later terminal is still recorded: its outcome is
/// `contained_task_deadline_exceeded` and carries `lease_expired`; the instance is fenced with
/// `lease_expired` as its cause; the Runtime runs; the other instance is not fenced and its direct
/// run completes.
#[test]
fn a64_d_the_sweep_leaves_the_end_alone_inside_the_grace_and_the_late_terminal_is_kept() {
    let fixture = fixture();
    let expiries = Arc::new(Mutex::new(Vec::new()));
    let (hook_host, hook_clock, hook_expiries) = (
        Arc::clone(&fixture.host),
        Arc::clone(&fixture.clock),
        Arc::clone(&expiries),
    );
    let lease_id = fixture.lease.lease_id();
    let end_work = fixture
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::EndWork,
            fixture.fenced,
            lease_id,
            move |_| {
                // The incident's tick: after the deadline plus the old reserve.
                hook_clock.advance(reserve_ms() + 1);
                hook_host
                    .expire_due_leases_for_test()
                    .expect("a sweep tick inside the end grace");
                hook_expiries
                    .lock()
                    .expect("expiries")
                    .push(lease_expiries(&hook_host, lease_id));
                // The end work runs past the end grace.
                hook_clock.advance(END_GRACE_MS);
                hook_host
                    .expire_due_leases_for_test()
                    .expect("a sweep tick after the end grace");
                hook_expiries
                    .lock()
                    .expect("expiries")
                    .push(lease_expiries(&hook_host, lease_id));
            },
        )
        .expect("arm the sweep ticks in the end work");

    let error = fixture.run_to_deadline();

    assert_eq!(end_work.consumed(), 1, "the run's end work ran");
    assert_eq!(
        *expiries.lock().expect("expiries"),
        vec![0, 1],
        "the sweep left the lease alone inside the end grace and expired it after"
    );
    fixture.assert_runtime_runs_and_returns_the_deadline(&error);
    let (_, codes) = fixture.deadline_terminal();
    assert!(
        codes
            .iter()
            .any(|code| code == RuntimeErrorCode::LeaseExpired.as_str()),
        "the late terminal says its lease had lapsed: {codes:?}"
    );
    assert_eq!(fixture.lease_expiries(), 1);
    assert_fenced_by_the_lapse(&fixture);
    fixture.assert_other_instance_runs();
    fixture.close();
}
